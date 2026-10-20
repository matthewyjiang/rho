use mermaid_rs_renderer::DiagramKind;

/// A diagram kind the terminal painter draws. Later stages match this
/// instead of [`DiagramKind`] so they never carry arms for unpainted kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PaintedKind {
    Flowchart,
    State,
    Class,
    Er,
    Sequence,
    GitGraph,
    Gantt,
    Mindmap,
}

/// Exhaustive paint-vs-dump gate for every diagram kind in mermaid-rs-renderer 0.3.1.
///
/// Keeping this match exhaustive makes dependency upgrades fail compilation until
/// each new Mermaid kind receives an explicit terminal rendering policy.
pub(super) const fn painted(kind: DiagramKind) -> Option<PaintedKind> {
    match kind {
        DiagramKind::Flowchart => Some(PaintedKind::Flowchart),
        DiagramKind::State => Some(PaintedKind::State),
        DiagramKind::Class => Some(PaintedKind::Class),
        DiagramKind::Er => Some(PaintedKind::Er),
        DiagramKind::Sequence => Some(PaintedKind::Sequence),
        DiagramKind::GitGraph => Some(PaintedKind::GitGraph),
        DiagramKind::Gantt => Some(PaintedKind::Gantt),
        DiagramKind::Mindmap => Some(PaintedKind::Mindmap),
        DiagramKind::Pie
        | DiagramKind::Journey
        | DiagramKind::Timeline
        | DiagramKind::Requirement
        | DiagramKind::C4
        | DiagramKind::Sankey
        | DiagramKind::Quadrant
        | DiagramKind::ZenUML
        | DiagramKind::Block
        | DiagramKind::Packet
        | DiagramKind::Kanban
        | DiagramKind::Architecture
        | DiagramKind::Radar
        | DiagramKind::Treemap
        | DiagramKind::XYChart => None,
    }
}
