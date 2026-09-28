//! Phase-3 GPU backend: **Metal** (macOS) acceleration for field sweeps.
//!
//! Compiled only with `--features gpu` on macOS (the `metal` dependency is
//! target-gated, so other platforms/CI never build it). It runs the 3-D field
//! stencil on the GPU and is therefore an **approximate f32 accelerator**: the
//! CPU interpreter is the f64 semantic oracle, and this path is opt-in
//! (`pwe run --gpu`) — results are within f32 tolerance, not bit-identical.
//! Everything that is not a supported sweep falls back to the CPU.
//!
//! The stencil arithmetic mirrors the CPU `field_diffuse` formula exactly (same
//! expression), so the only divergence is f64→f32 precision.

use pwe_api::{Error, Result, Status};
use std::ffi::c_void;

const MSL: &str = r#"
#include <metal_stdlib>
using namespace metal;
kernel void pwe_diffuse(device const float* cur  [[buffer(0)]],
                        device float*       out  [[buffer(1)]],
                        constant uint&      w    [[buffer(2)]],
                        constant uint&      h    [[buffer(3)]],
                        constant uint&      d    [[buffer(4)]],
                        constant float&     rate [[buffer(5)]],
                        constant float&     dx2  [[buffer(6)]],
                        uint3 gid [[thread_position_in_grid]]) {
  uint i = gid.x, j = gid.y, k = gid.z;
  if (i >= w || j >= h || k >= d) { return; }
  uint row = (k * h + j) * w;
  uint idx = row + i;
  float c = cur[idx];
  uint ui = (j > 0u) ? (row - w) : row;
  uint di = (j + 1u < h) ? (row + w) : row;
  uint bi = (k > 0u) ? (row - w * h) : row;
  uint fi = (k + 1u < d) ? (row + w * h) : row;
  float l  = (i > 0u) ? cur[row + i - 1u] : c;
  float r  = (i + 1u < w) ? cur[row + i + 1u] : c;
  float u  = cur[ui + i];
  float dn = cur[di + i];
  float b  = cur[bi + i];
  float f  = cur[fi + i];
  float lap = (l + r + u + dn + b + f - 6.0f * c) / dx2;
  out[idx] = c + rate * lap;
}
"#;

fn error(status: Status, detail: u32) -> Error {
    Error {
        status,
        detail,
        byte_offset: 0,
    }
}

/// A lazily-created Metal compute context for the field stencil.
pub struct Gpu {
    device: metal::Device,
    queue: metal::CommandQueue,
    pipeline: metal::ComputePipelineState,
}

impl Gpu {
    /// Creates the Metal device and compiles the stencil kernel. Errors (detail
    /// 5) if there is no Metal device or the kernel fails to compile.
    pub fn new() -> Result<Self> {
        let device = metal::Device::system_default().ok_or_else(|| error(Status::Invalid, 98))?;
        let queue = device.new_command_queue();
        let library = device
            .new_library_with_source(MSL, &metal::CompileOptions::new())
            .map_err(|_| error(Status::Invalid, 98))?;
        let function = library
            .get_function("pwe_diffuse", None)
            .map_err(|_| error(Status::Invalid, 98))?;
        let pipeline = device
            .new_compute_pipeline_state_with_function(&function)
            .map_err(|_| error(Status::Invalid, 98))?;
        Ok(Self {
            device,
            queue,
            pipeline,
        })
    }

    /// Human-readable adapter name (for diagnostics).
    pub fn name(&self) -> String {
        self.device.name().to_string()
    }

    /// Runs the diffuse stencil on the GPU for a `w×h×d` field. Input/output are
    /// f64 cells; the kernel computes in f32 (approximate).
    pub fn diffuse(
        &self,
        cur: &[f64],
        w: usize,
        h: usize,
        d: usize,
        rate: f64,
        dx2: f64,
    ) -> Result<Vec<f64>> {
        let n = cur.len();
        if n != w * h * d {
            return Err(error(Status::Invalid, 7));
        }
        let f32_in: Vec<f32> = cur.iter().map(|&v| v as f32).collect();
        let bytes = (n * std::mem::size_of::<f32>()) as u64;
        let opts = metal::MTLResourceOptions::StorageModeShared;
        let buf_in =
            self.device
                .new_buffer_with_data(f32_in.as_ptr() as *const c_void, bytes, opts);
        let buf_out = self.device.new_buffer(bytes, opts);
        let set = |enc: &metal::ComputeCommandEncoderRef, idx: u64, val: &u32| {
            enc.set_bytes(idx, 4, val as *const u32 as *const c_void);
        };
        let set_f = |enc: &metal::ComputeCommandEncoderRef, idx: u64, val: &f32| {
            enc.set_bytes(idx, 4, val as *const f32 as *const c_void);
        };
        let (wu, hu, du) = (w as u32, h as u32, d as u32);
        let ratef = rate as f32;
        let dx2f = dx2 as f32;

        let cmd = self.queue.new_command_buffer();
        let enc = cmd.new_compute_command_encoder();
        enc.set_compute_pipeline_state(&self.pipeline);
        enc.set_buffer(0, Some(&buf_in), 0);
        enc.set_buffer(1, Some(&buf_out), 0);
        set(enc, 2, &wu);
        set(enc, 3, &hu);
        set(enc, 4, &du);
        set_f(enc, 5, &ratef);
        set_f(enc, 6, &dx2f);
        let grid = metal::MTLSize {
            width: w as u64,
            height: h as u64,
            depth: d as u64,
        };
        let group = metal::MTLSize {
            width: 8,
            height: 8,
            depth: 1,
        };
        enc.dispatch_threads(grid, group);
        enc.end_encoding();
        cmd.commit();
        cmd.wait_until_completed();

        // SAFETY: the output buffer is `n` f32 and shared/unified memory; the
        // command buffer has completed, so the contents are visible.
        let out_f32: &[f32] =
            unsafe { std::slice::from_raw_parts(buf_out.contents() as *const f32, n) };
        Ok(out_f32.iter().map(|&v| v as f64).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_field_diffuse_run_matches_cpu_within_tolerance() {
        let mut gpu_rt = crate::lang::LangRuntime::compile(
            "world { gravity=(0,0,0)
               field u { width=16; height=16; dx=1.0 }
               entity probe { state=(0.0,0.0,0.0) } }
             systems {
               update  { on=probe; dt=0.1  fset(u, 8, 8, 1.0) }
               diffuse { field=u; rate=0.2 }
             }",
        )
        .unwrap();
        if gpu_rt.enable_gpu(true).is_err() {
            eprintln!("skipping: no Metal device");
            return;
        }
        let mut cpu_rt = crate::lang::LangRuntime::compile(
            "world { gravity=(0,0,0)
               field u { width=16; height=16; dx=1.0 }
               entity probe { state=(0.0,0.0,0.0) } }
             systems {
               update  { on=probe; dt=0.1  fset(u, 8, 8, 1.0) }
               diffuse { field=u; rate=0.2 }
             }",
        )
        .unwrap();
        for _ in 0..20 {
            cpu_rt.step_interpreter().unwrap();
            gpu_rt.step_interpreter().unwrap();
        }
        let cells = |rt: &crate::lang::LangRuntime| rt.scene.fields["u"].cells().to_vec();
        let (a, b) = (cells(&cpu_rt), cells(&gpu_rt));
        assert_eq!(a.len(), b.len());
        for (i, (c, g)) in a.iter().zip(b.iter()).enumerate() {
            assert!(
                (g - c).abs() <= 1e-3,
                "cell {i}: cpu={c} gpu={g} (divergence beyond f32 tolerance)"
            );
        }
        assert!(
            a.iter().any(|v| v.abs() > 1e-6),
            "field must be non-trivial"
        );
    }

    #[test]
    fn metal_diffuse_matches_cpu_within_f32_tolerance() {
        let gpu = match Gpu::new() {
            Ok(g) => g,
            Err(_) => {
                eprintln!("skipping: no Metal device");
                return;
            }
        };
        eprintln!("Metal device: {}", gpu.name());
        let (w, h, d) = (16usize, 12usize, 3usize);
        let n = w * h * d;
        let mut cur = vec![0.0f64; n];
        let mut s = 0x1234_5678_9abc_def0u64;
        for v in cur.iter_mut() {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            *v = (s as f64 / u64::MAX as f64) * 2.0 - 1.0;
        }
        let (rate, dx2) = (0.2, 1.0 / 64.0);
        let got = gpu.diffuse(&cur, w, h, d, rate, dx2).unwrap();
        // CPU reference (same formula, f64).
        let mut cpu = vec![0.0f64; n];
        for k in 0..d {
            for j in 0..h {
                for i in 0..w {
                    let idx = (k * h + j) * w + i;
                    let c = cur[idx];
                    let at = |ci: usize, cj: usize, ck: usize| cur[(ck * h + cj) * w + ci];
                    let l = if i > 0 { at(i - 1, j, k) } else { c };
                    let r = if i + 1 < w { at(i + 1, j, k) } else { c };
                    let u = if j > 0 { at(i, j - 1, k) } else { c };
                    let dn = if j + 1 < h { at(i, j + 1, k) } else { c };
                    let b = if k > 0 { at(i, j, k - 1) } else { c };
                    let f = if k + 1 < d { at(i, j, k + 1) } else { c };
                    let lap = (l + r + u + dn + b + f - 6.0 * c) / dx2;
                    cpu[idx] = c + rate * lap;
                }
            }
        }
        for (i, (g, c)) in got.iter().zip(cpu.iter()).enumerate() {
            assert!(
                (g - c).abs() <= 1e-4 * c.abs().max(1.0),
                "cell {i}: gpu={g} cpu={c}"
            );
        }
    }
}
