pub mod analysis;
pub mod color;
pub mod search;
pub mod snapshot;
pub mod storage;

pub use analysis::{
    analyze_target, format_analysis_report, render_analysis_mermaid, AnalysisReport,
};
pub use search::{
    format_search_hits, search_hits, search_surface_counts, SearchHit, SearchSurfaceCount,
};
pub use snapshot::{
    create_snapshot, list_snapshots, snapshot_count, snapshot_details, SnapshotDetails,
    SnapshotFileRecord, SnapshotOutcome, SnapshotRecord,
};
pub use storage::{
    append_event, connect_project_db, init_project, memorycore_dir, now_unix, project_db_path,
    ProjectLayout,
};
