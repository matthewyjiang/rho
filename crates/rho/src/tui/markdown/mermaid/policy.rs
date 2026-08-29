use mermaid_rs_renderer::DiagramKind;

/// Exhaustive paint-vs-dump gate for every diagram kind in mermaid-rs-renderer 0.3.1.
///
/// Keeping this match exhaustive makes dependency upgrades fail compilation until
/// each new Mermaid kind receives an explicit terminal rendering policy.
pub(super) const fn paints(kind: DiagramKind) -> bool {
    match kind {
        DiagramKind::Flowchart
        | DiagramKind::State
        | DiagramKind::Class
        | DiagramKind::Er
        | DiagramKind::Sequence
        | DiagramKind::GitGraph
        | DiagramKind::Gantt
        | DiagramKind::Mindmap => true,
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
        | DiagramKind::XYChart => false,
    }
}
