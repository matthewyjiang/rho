use mermaid_rs_renderer::DiagramKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DiagramPolicy {
    PaintFlow,
    PaintState,
    PaintClass,
    PaintEr,
    PaintSequence,
    RawFallback,
}

/// Exhaustive policy for every diagram kind exposed by mermaid-rs-renderer 0.3.1.
///
/// Keeping this match exhaustive makes dependency upgrades fail compilation until
/// each new Mermaid kind receives an explicit terminal rendering policy.
pub(super) const fn diagram_policy(kind: DiagramKind) -> DiagramPolicy {
    match kind {
        DiagramKind::Flowchart => DiagramPolicy::PaintFlow,
        DiagramKind::State => DiagramPolicy::PaintState,
        DiagramKind::Class => DiagramPolicy::PaintClass,
        DiagramKind::Er => DiagramPolicy::PaintEr,
        DiagramKind::Sequence => DiagramPolicy::PaintSequence,
        DiagramKind::Pie
        | DiagramKind::Mindmap
        | DiagramKind::Journey
        | DiagramKind::Timeline
        | DiagramKind::Gantt
        | DiagramKind::Requirement
        | DiagramKind::GitGraph
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
        | DiagramKind::XYChart => DiagramPolicy::RawFallback,
    }
}
