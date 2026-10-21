//! Turns a patch snapshot into a schedule: which modules run on the CPU, in
//! what order the GPU passes run, and which cables close a loop and therefore
//! carry last field's signal.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use dov_core::{Kernel, ModuleId, ParamBank, PortKind};
use dov_modules::ModuleType;

/// The patch as the engine sees it: module instances and resolved cables.
#[derive(Clone, Default)]
pub struct PatchSnapshot {
    pub modules: Vec<SnapshotModule>,
    pub cables: Vec<SnapshotCable>,
}

#[derive(Clone)]
pub struct SnapshotModule {
    pub id: ModuleId,
    pub ty: &'static ModuleType,
    pub params: Arc<ParamBank>,
}

#[derive(Clone, Copy, Debug)]
pub struct SnapshotCable {
    pub from: ModuleId,
    /// Output index on `from`.
    pub output: usize,
    pub to: ModuleId,
    /// Input index on `to`, counting generated CV inputs.
    pub input: usize,
}

/// Where an input gets its signal this field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Source {
    /// Unpatched: the port's default value.
    Default,
    Control {
        module: usize,
        output: usize,
    },
    /// A texture-backed signal (video or composite). `delayed`: read last
    /// field's value, because this cable closes a loop.
    Video {
        module: usize,
        output: usize,
        delayed: bool,
    },
    /// A video signal into a control input: its average brightness,
    /// measured on the GPU and read back a field or two late.
    Sampled {
        module: usize,
        output: usize,
    },
}

#[derive(Debug, Default)]
pub struct Schedule {
    /// Control-rate modules, upstream first.
    pub control: Vec<usize>,
    /// GPU passes, upstream first.
    pub gpu: Vec<usize>,
    /// Per module, per input (including CV inputs).
    pub inputs: Vec<Vec<Source>>,
    /// Outputs whose previous field must be kept for delayed cables.
    pub delayed_outputs: Vec<(usize, usize)>,
    /// Video outputs measured for control inputs.
    pub sampled_outputs: Vec<(usize, usize)>,
    /// The module whose signal goes to the output window.
    pub output: Option<usize>,
    pub warnings: Vec<String>,
}

pub const OUTPUT_MODULE: &str = "dov.output";

pub fn schedule(snap: &PatchSnapshot) -> Schedule {
    let index: HashMap<ModuleId, usize> = snap.modules.iter().enumerate().map(|(i, m)| (m.id, i)).collect();
    let mut s = Schedule {
        inputs: snap
            .modules
            .iter()
            .map(|m| vec![Source::Default; m.ty.desc.input_count()])
            .collect(),
        ..Default::default()
    };

    for cable in &snap.cables {
        let (Some(&from), Some(&to)) = (index.get(&cable.from), index.get(&cable.to)) else {
            s.warnings
                .push(format!("cable references a missing module: {cable:?}"));
            continue;
        };
        let (src, dst) = (&snap.modules[from].ty.desc, &snap.modules[to].ty.desc);
        let Some(out_port) = src.outputs.get(cable.output) else {
            s.warnings.push(format!("{}: no output {}", src.id, cable.output));
            continue;
        };
        let inputs = dst.inputs_full();
        let Some(in_port) = inputs.get(cable.input) else {
            s.warnings.push(format!("{}: no input {}", dst.id, cable.input));
            continue;
        };
        if !out_port.kind.can_drive(in_port.kind) {
            s.warnings.push(format!(
                "{}.{} ({:?}) can't drive {}.{} ({:?})",
                src.id, out_port.id, out_port.kind, dst.id, in_port.id, in_port.kind
            ));
            continue;
        }
        s.inputs[to][cable.input] = match (out_port.kind, in_port.kind) {
            (PortKind::Control, _) => Source::Control {
                module: from,
                output: cable.output,
            },
            (_, PortKind::Control) => {
                if !s.sampled_outputs.contains(&(from, cable.output)) {
                    s.sampled_outputs.push((from, cable.output));
                }
                Source::Sampled {
                    module: from,
                    output: cable.output,
                }
            }
            _ => Source::Video {
                module: from,
                output: cable.output,
                delayed: false,
            },
        };
    }

    let kernel = |i: usize| snap.modules[i].ty.desc.kernel;
    let is_control = |i: usize| matches!(kernel(i), Kernel::Control);
    let is_store = |i: usize| matches!(kernel(i), Kernel::Feedback);

    // Control modules: upstream first. A loop among them simply reads the
    // value from the previous field, so back edges are ignored.
    let mut state = vec![Visit::New; snap.modules.len()];
    for i in (0..snap.modules.len()).filter(|&i| is_control(i)) {
        visit_control(i, &s.inputs, &mut state, &mut s.control);
    }

    // Frame stores output last field's input, which is copied into the store
    // after every pass has run, so their main input is never a dependency and
    // loops through it need no delay. Their CV inputs are read this field.
    // Stores are visited last, so a loop back into a store's CV jack is
    // delayed there, and the picture path out of the store stays current.
    let mut state = vec![Visit::New; snap.modules.len()];
    let mut delayed = HashSet::new();
    let gpu = |store: bool| (0..snap.modules.len()).filter(move |&i| !is_control(i) && is_store(i) == store);
    for i in gpu(false).chain(gpu(true)) {
        visit_gpu(i, &mut s.inputs, &mut state, &mut s.gpu, &is_store, &mut delayed);
    }
    s.delayed_outputs = delayed.into_iter().collect();
    s.delayed_outputs.sort_unstable();

    s.output = snap.modules.iter().position(|m| m.ty.desc.id == OUTPUT_MODULE);
    s
}

#[derive(Clone, Copy, PartialEq)]
enum Visit {
    New,
    OnStack,
    Done,
}

fn visit_control(n: usize, inputs: &[Vec<Source>], state: &mut [Visit], order: &mut Vec<usize>) {
    if state[n] != Visit::New {
        return;
    }
    state[n] = Visit::OnStack;
    for src in &inputs[n] {
        if let Source::Control { module, .. } = *src {
            visit_control(module, inputs, state, order);
        }
    }
    state[n] = Visit::Done;
    order.push(n);
}

fn visit_gpu(
    n: usize,
    inputs: &mut [Vec<Source>],
    state: &mut [Visit],
    order: &mut Vec<usize>,
    is_store: &dyn Fn(usize) -> bool,
    delayed: &mut HashSet<(usize, usize)>,
) {
    if state[n] != Visit::New {
        return;
    }
    state[n] = Visit::OnStack;
    // A frame store's main input (input 0) is only read after this field.
    let first = usize::from(is_store(n));
    for slot in first..inputs[n].len() {
        let Source::Video { module, output, .. } = inputs[n][slot] else {
            continue;
        };
        match state[module] {
            // `module` is downstream of `n` and also feeds it: this cable
            // closes a loop, so it carries last field's signal.
            Visit::OnStack => {
                inputs[n][slot] = Source::Video {
                    module,
                    output,
                    delayed: true,
                };
                delayed.insert((module, output));
            }
            Visit::New => visit_gpu(module, inputs, state, order, is_store, delayed),
            Visit::Done => {}
        }
    }
    state[n] = Visit::Done;
    order.push(n);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(id: u64, kind: &str) -> SnapshotModule {
        let ty = dov_modules::find(kind).unwrap();
        SnapshotModule {
            id: ModuleId(id),
            ty,
            params: Arc::new(ParamBank::new(&ty.desc.default_slots())),
        }
    }

    fn cable(snap: &PatchSnapshot, from: u64, out: &str, to: u64, inp: &str) -> SnapshotCable {
        let desc = |id: u64| {
            &snap
                .modules
                .iter()
                .find(|m| m.id == ModuleId(id))
                .unwrap()
                .ty
                .desc
        };
        SnapshotCable {
            from: ModuleId(from),
            output: desc(from).output_index(out).unwrap(),
            to: ModuleId(to),
            input: desc(to).input_index(inp).unwrap(),
        }
    }

    fn position(order: &[usize], i: usize) -> usize {
        order.iter().position(|&x| x == i).unwrap()
    }

    #[test]
    fn chain_runs_upstream_first() {
        let mut snap = PatchSnapshot {
            modules: vec![
                module(3, "dov.output"),
                module(2, "dov.fold"),
                module(1, "dov.vco"),
            ],
            cables: vec![],
        };
        snap.cables = vec![cable(&snap, 1, "out", 2, "sig"), cable(&snap, 2, "out", 3, "sig")];
        let s = schedule(&snap);
        assert!(s.warnings.is_empty(), "{:?}", s.warnings);
        assert!(position(&s.gpu, 2) < position(&s.gpu, 1));
        assert!(position(&s.gpu, 1) < position(&s.gpu, 0));
        assert!(s.delayed_outputs.is_empty());
        assert_eq!(s.output, Some(0));
    }

    #[test]
    fn a_loop_gets_exactly_one_delayed_cable() {
        let mut snap = PatchSnapshot {
            modules: vec![module(1, "dov.mixer"), module(2, "dov.fold")],
            cables: vec![],
        };
        snap.cables = vec![cable(&snap, 1, "out", 2, "sig"), cable(&snap, 2, "out", 1, "a")];
        let s = schedule(&snap);
        assert_eq!(s.delayed_outputs.len(), 1);
        let delayed: Vec<_> = s
            .inputs
            .iter()
            .flatten()
            .filter(|src| matches!(src, Source::Video { delayed: true, .. }))
            .collect();
        assert_eq!(delayed.len(), 1);
        assert_eq!(s.gpu.len(), 2);
    }

    #[test]
    fn feedback_closes_loops_without_extra_delay() {
        let mut snap = PatchSnapshot {
            modules: vec![module(1, "dov.mixer"), module(2, "dov.feedback")],
            cables: vec![],
        };
        snap.cables = vec![cable(&snap, 1, "out", 2, "sig"), cable(&snap, 2, "out", 1, "a")];
        let s = schedule(&snap);
        assert!(s.delayed_outputs.is_empty());
        assert_eq!(s.gpu, [1, 0], "frame store runs first");
    }

    #[test]
    fn video_into_a_control_input_is_sampled() {
        let mut snap = PatchSnapshot {
            modules: vec![module(1, "dov.vco"), module(2, "dov.lfo")],
            cables: vec![],
        };
        snap.cables = vec![cable(&snap, 1, "out", 2, "rate_cv")];
        let s = schedule(&snap);
        assert!(s.warnings.is_empty(), "{:?}", s.warnings);
        assert_eq!(s.inputs[1][0], Source::Sampled { module: 0, output: 0 });
        assert_eq!(s.sampled_outputs, [(0, 0)]);
        assert_eq!(s.control, [1]);
    }

    #[test]
    fn feedback_cv_sources_run_first_and_loops_through_them_are_delayed() {
        let mut snap = PatchSnapshot {
            modules: vec![
                module(1, "dov.vco"),
                module(2, "dov.feedback"),
                module(3, "dov.colorize"),
            ],
            cables: vec![],
        };
        snap.cables = vec![
            cable(&snap, 1, "out", 2, "zoom_cv"),
            cable(&snap, 2, "out", 3, "sig"),
            cable(&snap, 3, "out", 2, "y_cv"),
            cable(&snap, 3, "out", 2, "sig"),
        ];
        let s = schedule(&snap);
        assert!(s.warnings.is_empty(), "{:?}", s.warnings);
        assert!(
            position(&s.gpu, 0) < position(&s.gpu, 1),
            "VCO before the store it modulates"
        );
        assert!(position(&s.gpu, 1) < position(&s.gpu, 2));
        let y_cv = snap.modules[1].ty.desc.input_index("y_cv").unwrap();
        assert!(matches!(s.inputs[1][y_cv], Source::Video { delayed: true, .. }));
        assert!(matches!(s.inputs[1][0], Source::Video { delayed: false, .. }));
        assert_eq!(s.delayed_outputs, [(2, 0)]);
    }

    #[test]
    fn control_runs_on_cpu_and_broadcasts_into_video() {
        let mut snap = PatchSnapshot {
            modules: vec![
                module(1, "dov.fold"),
                module(2, "dov.lfo"),
                module(3, "dov.macros"),
            ],
            cables: vec![],
        };
        snap.cables = vec![
            cable(&snap, 2, "out", 1, "gain_cv"),
            cable(&snap, 3, "a", 2, "rate_cv"),
        ];
        let s = schedule(&snap);
        assert!(s.warnings.is_empty(), "{:?}", s.warnings);
        assert_eq!(s.control, [2, 1], "macros before the LFO it modulates");
        assert_eq!(s.gpu, [0]);
        assert_eq!(s.inputs[0][1], Source::Control { module: 1, output: 0 });
    }
}
