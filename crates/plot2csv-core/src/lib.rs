//! Core data model and pure logic for plot2csv (no UI, no image processing).

pub mod axes;
pub mod dataset;
pub mod export;
pub mod geom;
pub mod history;
pub mod project;
pub mod table;
pub mod value;

pub use axes::{Axes, AxesKind, AxesParams, Calibration};
pub use dataset::{Algorithm, AutoSettings, ColorMode, DataPoint, Dataset, Mask, SeriesStyle};
pub use geom::{Homography, Pt, RectF};
pub use project::{Correction, Panel, PanelWork, Project, Stage, StageState, StageStatus, TextItem, TextRole};
pub use table::{SortBy, TableOptions};
pub use value::NumFormat;
