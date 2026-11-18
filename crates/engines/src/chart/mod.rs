//! Charts in Office files (DrawingML `c:chartSpace`), drawn from the values Office caches
//! in the chart part: bar and column (clustered, stacked, 100 %), line, area, pie,
//! doughnut and scatter, with titles, legends, axes, gridlines and data labels. Shared by
//! the PowerPoint, Excel and Word → PDF engines.

pub(crate) mod draw;
mod read;

pub(crate) use draw::draw;
pub use draw::{auto_color, nice_scale};
pub use read::{Axis, Chart, Grouping, Kind, LegendPos, Plot, Series, parse};

#[cfg(test)]
mod tests;
