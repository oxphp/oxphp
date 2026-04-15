//! PHP worker pool configuration.
//!
//! `WorkerMode` lives here (not in the executor) so `Config::from_env()`
//! can parse `PHP_WORKERS` exactly once and hand the result to
//! `SapiExecutor::new()`. Always compiled, including without the `php`
//! feature, so `cargo test --no-default-features` can exercise the parser.

use std::fmt;

/// Worker scaling mode parsed from `PHP_WORKERS` env var.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkerMode {
    /// Fixed number of workers.
    Static(usize),
    /// Dynamic scaling between min and max.
    Dynamic { min: usize, max: usize },
}

impl WorkerMode {
    /// Initial worker count: exact for static, min for dynamic.
    pub fn worker_count(&self) -> usize {
        match self {
            WorkerMode::Static(n) => *n,
            WorkerMode::Dynamic { min, .. } => *min,
        }
    }

    /// Maximum worker count: exact for static, max for dynamic.
    pub fn max_worker_count(&self) -> usize {
        match self {
            WorkerMode::Static(n) => *n,
            WorkerMode::Dynamic { max, .. } => *max,
        }
    }
}

impl fmt::Display for WorkerMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorkerMode::Static(n) => write!(f, "{n}"),
            WorkerMode::Dynamic { min, max } => write!(f, "{min}:{max}"),
        }
    }
}

/// Parse `PHP_WORKERS` env var into a `WorkerMode`.
///
/// Formats:
/// - `""` or `"0"` → Static(cpu/2, min 1)
/// - `"N"` → Static(N)
/// - `"MIN:MAX"` → Dynamic { min, max }
/// - `"0:0"` → Dynamic { min: cpu/4 (min 1), max: cpu*2 }
pub fn parse_php_workers(val: &str) -> Result<WorkerMode, String> {
    let cpu = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    if let Some((left, right)) = val.split_once(':') {
        if left.is_empty() || right.is_empty() {
            return Err(format!(
                "invalid PHP_WORKERS: '{val}' (both MIN and MAX required)"
            ));
        }
        let min_raw: usize = left.parse().map_err(|_| format!("invalid MIN: '{left}'"))?;
        let max_raw: usize = right
            .parse()
            .map_err(|_| format!("invalid MAX: '{right}'"))?;
        let min = if min_raw == 0 {
            (cpu / 4).max(1)
        } else {
            min_raw
        };
        let max = if max_raw == 0 { cpu * 2 } else { max_raw };
        if min > max {
            return Err(format!("PHP_WORKERS: min ({min}) > max ({max})"));
        }
        Ok(WorkerMode::Dynamic { min, max })
    } else {
        let n: usize = val
            .parse()
            .map_err(|_| format!("invalid PHP_WORKERS: '{val}'"))?;
        let count = if n == 0 { (cpu / 2).max(1) } else { n };
        Ok(WorkerMode::Static(count))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_static_positive() {
        assert_eq!(parse_php_workers("8").unwrap(), WorkerMode::Static(8));
        assert_eq!(parse_php_workers("1").unwrap(), WorkerMode::Static(1));
    }

    #[test]
    fn parse_static_zero_uses_default() {
        let cpu = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        assert_eq!(
            parse_php_workers("0").unwrap(),
            WorkerMode::Static((cpu / 2).max(1))
        );
    }

    #[test]
    fn parse_dynamic_explicit() {
        assert_eq!(
            parse_php_workers("2:16").unwrap(),
            WorkerMode::Dynamic { min: 2, max: 16 }
        );
        assert_eq!(
            parse_php_workers("4:4").unwrap(),
            WorkerMode::Dynamic { min: 4, max: 4 }
        );
    }

    #[test]
    fn parse_dynamic_auto_min_uses_default() {
        let cpu = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        assert_eq!(
            parse_php_workers("0:16").unwrap(),
            WorkerMode::Dynamic {
                min: (cpu / 4).max(1),
                max: 16,
            }
        );
    }

    #[test]
    fn parse_dynamic_auto_max_uses_default() {
        let cpu = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        assert_eq!(
            parse_php_workers("2:0").unwrap(),
            WorkerMode::Dynamic {
                min: 2,
                max: cpu * 2,
            }
        );
    }

    #[test]
    fn parse_dynamic_both_auto() {
        let cpu = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        assert_eq!(
            parse_php_workers("0:0").unwrap(),
            WorkerMode::Dynamic {
                min: (cpu / 4).max(1),
                max: cpu * 2,
            }
        );
    }

    #[test]
    fn parse_rejects_min_gt_max() {
        assert!(parse_php_workers("16:8").is_err());
    }

    #[test]
    fn parse_rejects_malformed() {
        assert!(parse_php_workers("abc").is_err());
        assert!(parse_php_workers(":8").is_err());
        assert!(parse_php_workers("8:").is_err());
        assert!(parse_php_workers("x:y").is_err());
    }

    #[test]
    fn worker_count_returns_min() {
        assert_eq!(WorkerMode::Static(4).worker_count(), 4);
        assert_eq!(WorkerMode::Dynamic { min: 2, max: 16 }.worker_count(), 2);
    }

    #[test]
    fn max_worker_count_returns_max() {
        assert_eq!(WorkerMode::Static(4).max_worker_count(), 4);
        assert_eq!(
            WorkerMode::Dynamic { min: 2, max: 16 }.max_worker_count(),
            16
        );
    }

    #[test]
    fn display_formats() {
        assert_eq!(WorkerMode::Static(4).to_string(), "4");
        assert_eq!(WorkerMode::Dynamic { min: 2, max: 16 }.to_string(), "2:16");
    }
}
