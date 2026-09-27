//! Cross-backend runtime: compile a program once, then step it with the
//! interpreter and the CPU JIT, requiring byte-identical writes each step.
use super::*;

// ---------------------------------------------------------------------------
// Cross-backend runtime
// ---------------------------------------------------------------------------

/// A declared `conserved { expr; tolerance }` quantity and its tracked drift.
struct ConservedSpec {
    expr: String,
    tolerance: f64,
    first: Option<f64>,
    last: f64,
}

/// Runs a compiled program's low-level IR across the interpreter and the CPU
/// JIT, verifying they produce byte-identical writes (the "run cross" contract).
pub struct LangRuntime {
    pub scene: Scene,
    pub program: PhysicsProgram,
    pub module: EirModule,
    pub clock: u64,
    /// Seconds advanced per step (the `update` system's dt, else 1/60).
    pub sim_dt: f64,
    /// This runtime's region id (for cross-runtime channel routing).
    pub region: pwe_api::RegionId,
    jit: CpuJit,
    jit_key: CodeCacheKey,
    /// Cross-runtime channel router backing the language `chan`s.
    router: crate::channel::ChannelRouter,
    /// The scene entity ids that are channels (their `state[0]` is the value).
    channel_ids: Vec<u128>,
    /// Entity id -> name, for presentation labels.
    entity_names: std::collections::BTreeMap<u128, String>,
    /// Field names that are solver-internal (`wave`'s `prev` time-shift buffer)
    /// and are not presented as physical fields.
    hidden_fields: std::collections::BTreeSet<String>,
    /// RFC-0040: soft-body render bonds (entity-id pairs).
    soft_bonds: Vec<(u128, u128)>,
    /// Optional peer region: when set, `send` also routes to the peer's channel.
    peer_region: Option<pwe_api::RegionId>,
    /// Execution context for `time`/`random`/`emit` (seeded → reproducible).
    env: crate::eir::ExecEnv,
    /// Raw invariant expressions in source order; verdict field `i` of the
    /// hidden check component belongs to `invariant_exprs[i]` (diagnostics).
    invariant_exprs: Vec<String>,
    /// When set, every step asserts that no entity state became non-finite and
    /// fails with detail 88 otherwise (catches unphysical blow-ups).
    finite_check: bool,
    /// Declared conserved quantities (drift tracked over the run; detail 87).
    conserved: Vec<ConservedSpec>,
}

impl LangRuntime {
    /// Compiles source and boots a runtime with an executable scene.
    pub fn compile(source: &str) -> Result<Self> {
        let compiled = compile(source)?;
        let scene = compiled.parsed.model.build_scene();
        Self::from_compiled_region(compiled, scene, RegionId(1))
    }

    /// Compiles a program file with its `import` fragments resolved.
    pub fn compile_file(path: &std::path::Path) -> Result<Self> {
        let compiled = compile_file(path)?;
        let scene = compiled.parsed.model.build_scene();
        Self::from_compiled_region(compiled, scene, RegionId(1))
    }

    /// Compiles source into a runtime belonging to `region` (for cross-runtime
    /// channel routing).
    pub fn compile_in_region(source: &str, region: RegionId) -> Result<Self> {
        let compiled = compile(source)?;
        let scene = compiled.parsed.model.build_scene();
        Self::from_compiled_region(compiled, scene, region)
    }

    /// Boots a runtime from an already-compiled program and a scene.
    pub fn from_compiled(compiled: CompiledProgram, scene: Scene) -> Result<Self> {
        Self::from_compiled_region(compiled, scene, RegionId(1))
    }

    /// Boots a runtime from a compiled program, scene, and owning region.
    pub fn from_compiled_region(
        compiled: CompiledProgram,
        scene: Scene,
        region: RegionId,
    ) -> Result<Self> {
        let module = compiled.eir.clone();
        let program = compiled.program;
        // The simulation clock advances by the `update` system's dt so `t`
        // tracks integration time; fall back to 1/60.
        let sim_dt = compiled
            .parsed
            .systems
            .iter()
            .find(|s| s.kind == "update")
            .and_then(|s| s.params.get("dt").copied())
            .unwrap_or(1.0 / 60.0);

        // Prepare a CPU JIT over the same low-level IR.
        let mut jit = CpuJit::new();
        jit.require_manifest(Hash256([7; 32]));
        jit.set_grants(Access::WRITE);
        let profile = profile_hash(b"pwe-lang");
        let code = jit.compile(&module, 0, profile, vec![JitAssumption::World(WorldId(0))])?;
        jit.publish(code)?;
        let jit_key = CodeCacheKey {
            module_hash: module.module_hash,
            target: 0,
            profile,
        };

        // Build the cross-runtime channel bridge: one router entry per language `chan`.
        let body_count = compiled.parsed.model.entities.len() as u128;
        let channel_ids: Vec<u128> = compiled
            .parsed
            .model
            .channels
            .iter()
            .enumerate()
            .map(|(i, _)| body_count + (i as u128) + 1)
            .collect();
        let mut entity_names: std::collections::BTreeMap<u128, String> = compiled
            .parsed
            .model
            .entities
            .iter()
            .enumerate()
            .map(|(i, e)| ((i as u128) + 1, e.name.clone()))
            .collect();
        for (i, c) in compiled.parsed.model.channels.iter().enumerate() {
            entity_names.insert(body_count + (i as u128) + 1, c.name.clone());
        }
        // RFC-0038: pool slots are named `<pool>#<k>`.
        for (name, base, count) in compiled.parsed.model.pool_ranges() {
            for k in 0..count {
                entity_names.insert(base + k as u128, format!("{name}#{k}"));
            }
        }
        // RFC-0040: soft particles are `<body>#<k>`.
        for (name, base, nx, ny, nz, _) in compiled.parsed.model.soft_ranges() {
            for k in 0..(nx * ny * nz) {
                entity_names.insert(base + k as u128, format!("{name}#{k}"));
            }
        }
        let mut router = ChannelRouter::new(region);
        for &cid in &channel_ids {
            router.channel(ChannelAddr::new(region, ChannelId(cid as u64)), 1);
        }

        // `wave`'s `prev` field is an internal time-shift buffer, not a
        // physical quantity — keep it out of the 3D view.
        let hidden_fields: std::collections::BTreeSet<String> = compiled
            .parsed
            .systems
            .iter()
            .filter(|s| s.kind == "wave")
            .filter_map(|s| s.string_params.get("prev").cloned())
            .collect();
        let soft_bonds = compiled.parsed.model.soft_bonds();
        Ok(Self {
            scene,
            program,
            module,
            clock: 0,
            sim_dt,
            hidden_fields,
            soft_bonds,
            region,
            jit,
            jit_key,
            router,
            channel_ids,
            entity_names,
            peer_region: None,
            env: crate::eir::ExecEnv::default(),
            finite_check: false,
            conserved: compiled
                .parsed
                .systems
                .iter()
                .filter(|s| s.kind == "conserved")
                .map(|s| ConservedSpec {
                    expr: s
                        .assigns
                        .get("expr")
                        .or_else(|| s.update.get("expr"))
                        .cloned()
                        .unwrap_or_default(),
                    tolerance: s.params.get("tolerance").copied().unwrap_or(1e-4),
                    first: None,
                    last: 0.0,
                })
                .collect(),
            invariant_exprs: compiled
                .parsed
                .systems
                .iter()
                .filter(|s| s.kind == "invariant")
                .filter_map(|s| {
                    s.assigns
                        .get("expr")
                        .or_else(|| s.update.get("expr"))
                        .cloned()
                })
                .collect(),
        })
    }

    /// Configures the peer region; when set, `send` also routes the value to the
    /// peer's channel (which the peer can `ingest` and `recv`).
    pub fn set_peer(&mut self, peer: RegionId) {
        self.peer_region = Some(peer);
    }

    /// Events emitted by the language `emit(...)` during the most recent step,
    /// in deterministic order.
    pub fn emitted_events(&self) -> &[crate::eir::EmittedEvent] {
        &self.env.events
    }

    /// Values logged by the language `print(...)` during the most recent step,
    /// in execution order (a debugging side-channel).
    pub fn logs(&self) -> &[String] {
        &self.env.log
    }

    /// Drains and returns the values logged by `print(...)` since the last call.
    pub fn drain_logs(&mut self) -> Vec<String> {
        std::mem::take(&mut self.env.log)
    }

    /// Pushes every channel's current value to the peer region's outbox (the
    /// cross-runtime / network transport).
    fn publish_channels(&mut self) {
        let Some(peer) = self.peer_region else {
            return;
        };
        for &cid in &self.channel_ids {
            let value = self
                .scene
                .get(EntityId(cid))
                .and_then(|e| e.state.as_ref())
                .map(|s| s.values[0])
                .unwrap_or(0.0);
            let _ = self.router.send(
                ChannelAddr::new(peer, ChannelId(cid as u64)),
                &value.to_bits().to_le_bytes(),
            );
        }
    }

    /// Serializes this runtime's outbound channel messages (the wire transport).
    pub fn transport_out(&self) -> Result<Vec<u8>> {
        self.router.encode_outbox()
    }

    /// Ingests a wire document from a peer, writing delivered messages into this
    /// runtime's channel entities so `recv` observes them.
    pub fn ingest(&mut self, bytes: &[u8]) -> Result<usize> {
        let delivered = self.router.ingest(bytes)?;
        for &cid in &self.channel_ids {
            let addr = ChannelAddr::new(self.region, ChannelId(cid as u64));
            if let Some(msg) = self.router.recv(addr)? {
                if msg.len() == 8 {
                    let value = f64::from_bits(u64::from_le_bytes(
                        msg[..8].try_into().expect("length-checked slice"),
                    ));
                    if let Some(e) = self.scene.get_mut(EntityId(cid)) {
                        if let Some(st) = e.state.as_mut() {
                            st.values[0] = value;
                        }
                    }
                }
            }
        }
        Ok(delivered)
    }

    /// Exchanges channel messages with a peer runtime over the wire (both ways).
    pub fn exchange(&mut self, peer: &mut LangRuntime) -> Result<()> {
        let to_peer = self.router.encode_outbox()?;
        let to_self = peer.router.encode_outbox()?;
        self.ingest(&to_self)?;
        peer.ingest(&to_peer)?;
        Ok(())
    }

    /// A presentation frame for the 3D web viewer: bodies rendered as entities
    /// (position from transform or state), and declared channels reported as
    /// channel values.
    pub fn present_frame(
        &self,
        camera: Option<crate::present::CameraVisual>,
    ) -> crate::present::PresentationFrame {
        let mut frame = crate::present::snapshot_with(
            &self.entity_names,
            &self.channel_ids,
            &self.scene,
            camera,
        );
        if !self.hidden_fields.is_empty() {
            frame
                .fields
                .retain(|f| !self.hidden_fields.contains(&f.name));
        }
        if !self.soft_bonds.is_empty() {
            frame.bonds = self.soft_bonds.clone();
        }
        frame
    }

    /// Resets the runtime to a previously captured scene (step 0, cleared
    /// execution context). Used by the live viewer's Restart.
    pub fn reset_to(&mut self, scene: Scene) {
        self.scene = scene;
        self.clock = 0;
        self.env = crate::eir::ExecEnv::default();
    }

    /// Enables/disables the per-step non-finite state check (detail 88).
    pub fn set_finite_check(&mut self, on: bool) {
        self.finite_check = on;
    }

    /// Fails (detail 88) when any entity's state holds a non-finite value.
    pub fn assert_finite(&self) -> Result<()> {
        for (id, e) in &self.scene.entities {
            let id = id.0;
            if let Some(st) = &e.state {
                for (i, v) in st.values.iter().enumerate() {
                    if !v.is_finite() {
                        let name = self
                            .entity_names
                            .get(&id)
                            .cloned()
                            .unwrap_or_else(|| format!("#{id}"));
                        return Err(error_at(
                            Status::EirInvalid,
                            88,
                            0,
                            format!("entity `{name}` slot {i} is non-finite ({v})"),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// Tracks each declared conserved quantity: the first value is the reference;
    /// each step's drift must stay within `tolerance` (detail 87).
    fn check_conserved(&mut self, writes: &[WorldWrite]) -> Result<()> {
        if self.conserved.is_empty() {
            return Ok(());
        }
        for w in writes {
            if w.component != crate::physics_eir::conserved_id() {
                continue;
            }
            let idx = (w.offset / crate::physics_eir::field::STATE_SLOT_BYTES) as usize;
            let Some(spec) = self.conserved.get_mut(idx) else {
                continue;
            };
            let v = f64::from_bits(w.value);
            match spec.first {
                None => {
                    spec.first = Some(v);
                    spec.last = v;
                }
                Some(first) => {
                    spec.last = v;
                    let drift = (v - first).abs();
                    let scale = first.abs().max(1.0);
                    if drift > spec.tolerance * scale {
                        return Err(error_at(
                            Status::EirInvalid,
                            87,
                            0,
                            format!(
                                "conserved quantity `{}` drifted: {first} -> {v} (Δ={drift}, tol={})",
                                spec.expr, spec.tolerance
                            ),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// `(expression, relative drift)` for each declared conserved quantity.
    pub fn conserved_drifts(&self) -> Vec<(String, f64)> {
        self.conserved
            .iter()
            .filter_map(|s| {
                let first = s.first?;
                let scale = first.abs().max(1.0);
                Some((s.expr.clone(), (s.last - first).abs() / scale))
            })
            .collect()
    }

    /// Advances the global simulation clock by one step.
    fn advance_clock(&mut self) {
        self.scene.sim_time += self.sim_dt;
        self.clock += 1;
        self.publish_channels();
    }

    /// Fails the step when any invariant verdict write is 1 — the invariant's
    /// expression evaluated to zero or NaN for some entity as the systems left
    /// the state. Verdict fields map back to their invariant expressions in
    /// source order for diagnostics.
    fn check_invariants(&self, writes: &[WorldWrite]) -> Result<()> {
        for w in writes {
            if w.component == crate::physics_eir::check_id() && f64::from_bits(w.value) >= 0.5 {
                let idx = (w.offset / crate::physics_eir::field::STATE_SLOT_BYTES) as usize;
                let desc = self
                    .invariant_exprs
                    .get(idx)
                    .map(String::as_str)
                    .unwrap_or("unknown");
                return Err(error_at(
                    Status::EirInvalid,
                    69,
                    0,
                    format!("invariant '{desc}' violated for entity {}", w.entity),
                ));
            }
        }
        Ok(())
    }

    /// Interpreter backend step: run the EIR, apply the ordered writes.
    pub fn step_interpreter(&mut self) -> Result<Vec<WorldWrite>> {
        let mut rt = SceneRuntime::new(&self.scene);
        self.env.time = self.scene.sim_time;
        self.env.step = self.clock;
        self.env.step_dt = self.sim_dt;
        self.env.events.clear();
        crate::eir::drain_due_events(&mut self.env);
        // The module was validated once at compile; executing skips the
        // dominance re-check every step (vital for unrolled field solvers).
        let writes = self
            .module
            .execute(&mut rt, &mut self.env, WorldId(0), WorldVersion(0))?;
        self.check_invariants(&writes)?;
        let overlays = rt.take_overlays();
        apply_writes(&mut self.scene, &writes)?;
        crate::physics_eir::flush_overlays(&mut self.scene, &overlays);
        if self.finite_check {
            self.assert_finite()?;
        }
        self.check_conserved(&writes)?;
        self.advance_clock();
        Ok(writes)
    }

    /// CPU JIT backend step over the same low-level IR.
    pub fn step_jit(&mut self) -> Result<Vec<WorldWrite>> {
        let mut rt = SceneRuntime::new(&self.scene);
        self.env.time = self.scene.sim_time;
        self.env.step = self.clock;
        self.env.step_dt = self.sim_dt;
        self.env.events.clear();
        crate::eir::drain_due_events(&mut self.env);
        let writes = self.jit.execute_with_env_validated(
            &self.jit_key,
            &mut rt,
            &mut self.env,
            WorldId(0),
            WorldVersion(0),
        )?;
        self.check_invariants(&writes)?;
        let overlays = rt.take_overlays();
        apply_writes(&mut self.scene, &writes)?;
        crate::physics_eir::flush_overlays(&mut self.scene, &overlays);
        self.advance_clock();
        Ok(writes)
    }

    /// Cross-backend step: run the interpreter and the JIT on identical scene
    /// clones against an identical seeded execution context and require
    /// byte-identical writes, then apply one authoritative result. This is the
    /// "run cross" guarantee — both backends share EIR semantics and must agree,
    /// including for `time`/`random`/`emit`.
    pub fn step_cross(&mut self) -> Result<Vec<WorldWrite>> {
        let a = self.scene.clone();
        let b = self.scene.clone();
        let base = {
            let mut e = self.env.clone();
            e.time = self.scene.sim_time;
            e.step = self.clock;
            e.step_dt = self.sim_dt;
            e.events.clear();
            crate::eir::drain_due_events(&mut e);
            e
        };

        let mut env_a = base.clone();
        let mut rt_a = SceneRuntime::new(&a);
        let int_writes = self
            .module
            .execute(&mut rt_a, &mut env_a, WorldId(0), WorldVersion(0))?;

        let mut env_b = base.clone();
        let mut rt_b = SceneRuntime::new(&b);
        let jit_writes = self.jit.execute_with_env_validated(
            &self.jit_key,
            &mut rt_b,
            &mut env_b,
            WorldId(0),
            WorldVersion(0),
        )?;

        if int_writes != jit_writes {
            return Err(error(Status::EirInvalid, 50));
        }
        // Both backends share the emit/schedule contract: their event streams
        // and dynamic event queues must agree too.
        if env_a.events != env_b.events || env_a.queue != env_b.queue {
            return Err(error(Status::EirInvalid, 50));
        }
        // RFC-0037: bulk field sweeps live in the dense overlays rather than the
        // write list, so require byte-identical overlays too.
        if rt_a.field_overlays() != rt_b.field_overlays() {
            return Err(error(Status::EirInvalid, 50));
        }
        let overlays = rt_a.take_overlays();
        // An invariant violation fails the step before any write is applied.
        self.check_invariants(&int_writes)?;
        // Advance the live env to match the interpreter's consumed random state,
        // so random streams accumulate deterministically across steps.
        self.env = env_a;
        apply_writes(&mut self.scene, &int_writes)?;
        crate::physics_eir::flush_overlays(&mut self.scene, &overlays);
        if self.finite_check {
            self.assert_finite()?;
        }
        // Copy the conserved writes before the borrow ends.
        let cw: Vec<WorldWrite> = int_writes.clone();
        self.check_conserved(&cw)?;
        self.advance_clock();
        Ok(int_writes)
    }

    pub fn step_cross_n(&mut self, n: u64) -> Result<()> {
        for _ in 0..n {
            self.step_cross()?;
        }
        Ok(())
    }

    /// Batched cross-backend stepping: `k-1` interpreter-only steps followed by
    /// one cross-verified step, so the JIT runs once per `k` steps rather than
    /// every step. Steady-state cost approaches a single backend while still
    /// cross-checking every `k` steps. `step_cross`/`step_cross_n` stay strict
    /// (per-step verification) for tests and conformance.
    pub fn step_cross_batched(&mut self, k: u32) -> Result<()> {
        let k = k.max(1);
        for _ in 1..k {
            self.step_interpreter()?;
        }
        self.step_cross()?;
        Ok(())
    }
}
