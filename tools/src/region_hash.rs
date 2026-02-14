use sha2::{Digest, Sha256};
use std::io::{self, BufRead};
use std::path::Path;

use crate::ObservedRegion;

/// Hash lines [start, end] inclusive (1-indexed).
pub fn hash_line_range(path: &Path, start: u32, end: u32) -> io::Result<[u8; 32]> {
    if start > end {
        return Ok(ObservedRegion::EMPTY_HASH);
    }

    let file = std::fs::File::open(path)?;
    let reader = io::BufReader::new(file);
    let mut hasher = Sha256::new();

    for (idx, line_result) in reader.lines().enumerate() {
        let line_num = (idx + 1) as u32;
        if line_num > end {
            break;
        }
        if line_num >= start {
            hasher.update(line_result?.as_bytes());
            hasher.update(b"\n");
        }
    }

    Ok(hasher.finalize().into())
}

/// Hash lines from already-read bytes [start, end] inclusive (1-indexed).
pub fn hash_line_range_bytes(bytes: &[u8], start: u32, end: u32) -> [u8; 32] {
    if start > end {
        return ObservedRegion::EMPTY_HASH;
    }

    let reader = io::BufReader::new(bytes);
    let mut hasher = Sha256::new();

    for (idx, line_result) in reader.lines().enumerate() {
        let line_num = (idx + 1) as u32;
        if line_num > end {
            break;
        }
        if line_num >= start
            && let Ok(line) = line_result
        {
            hasher.update(line.as_bytes());
            hasher.update(b"\n");
        }
    }

    hasher.finalize().into()
}

/// Create an observed region for a read operation.
pub fn create_region(path: &Path, start_line: u32, end_line: u32) -> io::Result<ObservedRegion> {
    let prefix_hash = if start_line > 1 {
        hash_line_range(path, 1, start_line - 1)?
    } else {
        ObservedRegion::EMPTY_HASH
    };
    let region_hash = hash_line_range(path, start_line, end_line)?;

    Ok(ObservedRegion {
        start_line,
        end_line,
        prefix_hash,
        region_hash,
    })
}

/// Merge two regions into one that covers both.
/// LLMs read files in chunks. This merges aggressively:
/// - Read 1-50, then 40-100 → merged region 1-100
/// - Read 100-150, then 1-50 → merged region 1-150
/// - Read 10-20, then 80-90 → merged region 10-90 (covers gap)
pub fn merge_regions(
    path: &Path,
    existing: &ObservedRegion,
    new_start: u32,
    new_end: u32,
) -> io::Result<ObservedRegion> {
    let merged_start = existing.start_line.min(new_start);
    let merged_end = existing.end_line.max(new_end);
    create_region(path, merged_start, merged_end)
}

#[derive(Debug, Clone)]
pub enum ValidationError {
    OutsideObservedRegion {
        target: u32,
        observed_start: u32,
        observed_end: u32,
    },
    PrefixChanged {
        above_line: u32,
    },
    RegionChanged,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutsideObservedRegion {
                target,
                observed_start,
                observed_end,
            } => {
                write!(
                    f,
                    "Edit targets line {target} but only lines {observed_start}-{observed_end} were read"
                )
            }
            Self::PrefixChanged { above_line } => {
                write!(
                    f,
                    "Content above line {above_line} changed since last read (lines may have shifted)"
                )
            }
            Self::RegionChanged => {
                write!(f, "File content changed since last read")
            }
        }
    }
}

/// Validate that an edit is permitted given the observed region.
/// Uses already-read bytes to avoid TOCTOU race.
pub fn validate_edit(
    original_bytes: &[u8],
    target_line: u32,
    region: &ObservedRegion,
) -> Result<(), ValidationError> {
    // Check edit is within observed bounds
    if target_line < region.start_line || target_line > region.end_line {
        return Err(ValidationError::OutsideObservedRegion {
            target: target_line,
            observed_start: region.start_line,
            observed_end: region.end_line,
        });
    }

    // Check prefix unchanged (detects insertions/deletions above)
    if region.start_line > 1 {
        let current_prefix = hash_line_range_bytes(original_bytes, 1, region.start_line - 1);
        if current_prefix != region.prefix_hash {
            return Err(ValidationError::PrefixChanged {
                above_line: region.start_line,
            });
        }
    }

    // Check region content unchanged
    let current_region = hash_line_range_bytes(original_bytes, region.start_line, region.end_line);
    if current_region != region.region_hash {
        return Err(ValidationError::RegionChanged);
    }

    Ok(())
}

/// Rehash a region after an edit (content changed, bounds same).
pub fn rehash_region_bytes(new_bytes: &[u8], region: &ObservedRegion) -> ObservedRegion {
    let prefix_hash = if region.start_line > 1 {
        hash_line_range_bytes(new_bytes, 1, region.start_line - 1)
    } else {
        ObservedRegion::EMPTY_HASH
    };
    let region_hash = hash_line_range_bytes(new_bytes, region.start_line, region.end_line);

    ObservedRegion {
        start_line: region.start_line,
        end_line: region.end_line,
        prefix_hash,
        region_hash,
    }
}
