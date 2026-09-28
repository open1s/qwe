//! Phase-3 GPU execution verification on macOS **Metal**.
//!
//! Runs the WGSL compute shaders emitted by `pwe_reference::wgsl` on the real
//! GPU through wgpu's Metal backend, and compares the results with the CPU
//! oracle / f64 interpreter (within f32 tolerance), including the RFC-0021 trap
//! flag.
//!
//! Run: `cargo run --release --manifest-path gpu-verify/Cargo.toml`.
//! This crate is standalone (its own workspace) so the main workspace/CI does
//! not build wgpu.

use pwe_reference::lang::{compile_program, parse};
use pwe_reference::wgsl::{emit_compute_shader, eval_f32};

fn kernel(body: &str) -> (pwe_reference::eir::EirModule, u64) {
    let src = format!(
        "world {{ gravity=(0,0,0) entity e {{ state=(a=0.0, x=0.0) }} }} \
         funcs {{ f(a) {{ {body} }} }} \
         systems {{ update {{ on=e; dt=1.0  x = f(a) }} }}"
    );
    let module = compile_program(parse(&src).unwrap())
        .unwrap()
        .eir
        .optimize();
    (module, 0xF000_0000u64)
}

/// Runs the shader over `inputs` on the GPU, returning `(outputs, trap)`.
fn run_on_metal(shader: &str, inputs: &[f32]) -> (Vec<f32>, u32) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::METAL,
        flags: wgpu::InstanceFlags::default(),
        memory_budget_thresholds: Default::default(),
        backend_options: Default::default(),
        display: None,
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .expect("no Metal adapter");
    let info = adapter.get_info();
    println!("GPU adapter: {} ({:?})", info.name, info.backend);
    assert_eq!(
        info.backend,
        wgpu::Backend::Metal,
        "expected the Metal backend"
    );

    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("gpu-verify"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::downlevel_defaults(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("no Metal device");

    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("pwe-kernel"),
        source: wgpu::ShaderSource::Wgsl(shader.into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("pwe-pipeline"),
        layout: None,
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });

    let n = inputs.len();
    let bytes = (n * 4) as u64;
    let input_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("input"),
        size: bytes,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let output_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("output"),
        size: bytes,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let trap_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("trap"),
        size: 4,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let out_stage = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("out-stage"),
        size: bytes,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let trap_stage = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("trap-stage"),
        size: 4,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(
        &input_buf,
        0,
        &inputs
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<u8>>(),
    );
    queue.write_buffer(&trap_buf, 0, &0u32.to_le_bytes());

    let bgl = pipeline.get_bind_group_layout(0);
    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: input_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: output_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: trap_buf.as_entire_binding(),
            },
        ],
    });

    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: None,
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bg, &[]);
        pass.dispatch_workgroups((n as u32).div_ceil(64), 1, 1);
    }
    enc.copy_buffer_to_buffer(&output_buf, 0, &out_stage, 0, bytes);
    enc.copy_buffer_to_buffer(&trap_buf, 0, &trap_stage, 0, 4);
    queue.submit(Some(enc.finish()));

    let read = |buf: &wgpu::Buffer, len: u64| -> Vec<u8> {
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();
        rx.recv().unwrap().unwrap();
        let data = slice.get_mapped_range().unwrap().to_vec();
        buf.unmap();
        assert_eq!(data.len() as u64, len);
        data
    };
    let out_bytes = read(&out_stage, bytes);
    let trap = u32::from_le_bytes(read(&trap_stage, 4).try_into().unwrap());
    let out = out_bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    (out, trap)
}

fn main() {
    // 1) A benign kernel: f(a) = a*a + sin(a).
    let (module, id) = kernel("a * a + sin(a)");
    let shader = emit_compute_shader(&module, id).unwrap();
    let inputs: Vec<f32> = (0..64).map(|i| i as f32 * 0.1 - 3.0).collect();
    let (out, trap) = run_on_metal(&shader, &inputs);
    assert_eq!(trap, 0, "benign kernel must not trap");
    for (i, &a) in inputs.iter().enumerate() {
        let oracle = eval_f32(&module, id, a).unwrap();
        let tol = 1e-4 * oracle.abs().max(1.0);
        assert!(
            (out[i] - oracle).abs() <= tol,
            "i={i} a={a}: gpu={} oracle={oracle}",
            out[i]
        );
    }
    println!(
        "OK: benign kernel on Metal matches the oracle ({} lanes)",
        inputs.len()
    );

    // 2) A trapping kernel: f(a) = 1 / a, with a zero input -> trap flag = 1.
    let (module, id) = kernel("1.0 / a");
    let shader = emit_compute_shader(&module, id).unwrap();
    let mut inputs: Vec<f32> = (0..64).map(|i| i as f32 + 1.0).collect();
    inputs[7] = 0.0;
    let (_out, trap) = run_on_metal(&shader, &inputs);
    assert_eq!(trap, 1, "div-by-zero must set the device trap flag");
    println!("OK: RFC-0021 div-by-zero trap flag set on Metal");

    println!("WGSL-on-Metal verification passed.");
}
