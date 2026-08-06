use crate::tui::markdown::txm::layout_tree::LayoutNode;
use crate::tui::markdown::txm::style::Style;

pub trait Backend {
    type Output;
    type Error: std::error::Error;

    fn render(&self, tree: &LayoutNode) -> Result<Self::Output, Self::Error>;
}

pub trait RenderTarget {
    fn set(&mut self, x: usize, y: usize, ch: char, style: Style);
    fn fill_row(&mut self, y: usize, x_start: usize, x_end: usize, ch: char, style: Style);
}
