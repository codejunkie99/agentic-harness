//! HarnessScore evaluation for Agentic Harness coding runs.
//!
//! The crate is intentionally independent from the CLI. It reads stable run
//! artifacts, converts them into a scoring input, computes pure metrics, and
//! renders reports.

mod input;
mod legacy_logs;
mod metrics;
mod native_run;
mod report;
mod weights;

pub use input::{
    RepairTelemetry, ScoreAction, ScoreArtifacts, ScoreCheck, ScoreDiff, ScoreInput,
    ScoreMetadata, ScoreMetricBreakdown, ScorePhase, ScoreResult, ScoreSource, ScoreStats,
};
pub use legacy_logs::score_legacy_logs;
pub use metrics::score;
pub use native_run::{score_native_run, score_native_run_files};
pub use report::render_markdown;
pub use weights::ScoreWeights;

use std::path::PathBuf;
use thiserror::Error;

const MAX_ARTIFACT_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum ScoreError {
    #[error("score artifact {path} was not found")]
    MissingArtifact { path: PathBuf },
    #[error("score artifact {path} is {size} bytes, above the limit of {limit} bytes")]
    ArtifactTooLarge {
        path: PathBuf,
        size: u64,
        limit: u64,
    },
    #[error("invalid score artifact {path}: {message}")]
    InvalidArtifact { path: PathBuf, message: String },
    #[error("I/O error while reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("JSON error in {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

fn read_capped(path: &std::path::Path) -> Result<String, ScoreError> {
    let metadata = std::fs::metadata(path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            ScoreError::MissingArtifact {
                path: path.to_path_buf(),
            }
        } else {
            ScoreError::Io {
                path: path.to_path_buf(),
                source,
            }
        }
    })?;
    if metadata.len() > MAX_ARTIFACT_BYTES {
        return Err(ScoreError::ArtifactTooLarge {
            path: path.to_path_buf(),
            size: metadata.len(),
            limit: MAX_ARTIFACT_BYTES,
        });
    }
    std::fs::read_to_string(path).map_err(|source| ScoreError::Io {
        path: path.to_path_buf(),
        source,
    })
}
