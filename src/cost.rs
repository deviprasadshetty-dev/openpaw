use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub cost_usd: f64,
    pub timestamp_secs: i64,
}

impl TokenUsage {
    pub fn new(
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
        input_price_per_million: f64,
        output_price_per_million: f64,
    ) -> Self {
        let safe_input_price =
            if input_price_per_million.is_finite() && input_price_per_million > 0.0 {
                input_price_per_million
            } else {
                0.0
            };
        let safe_output_price =
            if output_price_per_million.is_finite() && output_price_per_million > 0.0 {
                output_price_per_million
            } else {
                0.0
            };

        let total = input_tokens.saturating_add(output_tokens);
        let input_cost = (input_tokens as f64 / 1_000_000.0) * safe_input_price;
        let output_cost = (output_tokens as f64 / 1_000_000.0) * safe_output_price;

        Self {
            model: model.to_string(),
            input_tokens,
            output_tokens,
            total_tokens: total,
            cost_usd: input_cost + output_cost,
            timestamp_secs: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UsagePeriod {
    Session,
    Day,
    Month,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetInfo {
    pub current_usd: f64,
    pub limit_usd: f64,
    pub period: UsagePeriod,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BudgetCheck {
    Allowed,
    Warning(BudgetInfo),
    Exceeded(BudgetInfo),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostRecord {
    pub usage: TokenUsage,
    pub session_id: String,
}

pub struct CostTracker {
    enabled: bool,
    daily_limit_usd: f64,
    monthly_limit_usd: f64,
    warn_at_percent: u32,
    session_records: Vec<CostRecord>,
    total_session_cost: f64,
    storage_path: PathBuf,
}

impl CostTracker {
    pub fn init(
        workspace_dir: &str,
        enabled: bool,
        daily_limit: f64,
        monthly_limit: f64,
        warn_pct: u32,
    ) -> Self {
        let path = PathBuf::from(workspace_dir)
            .join("state")
            .join("costs.jsonl");
        Self {
            enabled,
            daily_limit_usd: daily_limit,
            monthly_limit_usd: monthly_limit,
            warn_at_percent: warn_pct,
            session_records: Vec::new(),
            total_session_cost: 0.0,
            storage_path: path,
        }
    }

    pub fn session_cost(&self) -> f64 {
        self.total_session_cost
    }

    pub fn check_budget(&self, estimated_cost_usd: f64) -> BudgetCheck {
        if !self.enabled {
            return BudgetCheck::Allowed;
        }
        if !estimated_cost_usd.is_finite() || estimated_cost_usd < 0.0 {
            return BudgetCheck::Allowed;
        }

        let session_cost = self.session_cost();
        let projected = session_cost + estimated_cost_usd;

        if projected > self.daily_limit_usd {
            return BudgetCheck::Exceeded(BudgetInfo {
                current_usd: session_cost,
                limit_usd: self.daily_limit_usd,
                period: UsagePeriod::Day,
            });
        }

        if projected > self.monthly_limit_usd {
            return BudgetCheck::Exceeded(BudgetInfo {
                current_usd: session_cost,
                limit_usd: self.monthly_limit_usd,
                period: UsagePeriod::Month,
            });
        }

        let warn_threshold = (self.warn_at_percent.min(100) as f64) / 100.0;
        let daily_warn = self.daily_limit_usd * warn_threshold;

        if projected >= daily_warn {
            return BudgetCheck::Warning(BudgetInfo {
                current_usd: session_cost,
                limit_usd: self.daily_limit_usd,
                period: UsagePeriod::Day,
            });
        }

        BudgetCheck::Allowed
    }

    pub fn record_usage(&mut self, usage: TokenUsage) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        if !usage.cost_usd.is_finite() || usage.cost_usd < 0.0 {
            return Ok(());
        }

        self.total_session_cost += usage.cost_usd;

        let record = CostRecord {
            usage,
            session_id: "current".to_string(),
        };
        self.session_records.push(record.clone());

        // Keep only recent 1000 records to prevent memory leak
        if self.session_records.len() > 1000 {
            self.session_records.remove(0);
        }

        self.append_to_jsonl(&record)?;
        Ok(())
    }

    fn append_to_jsonl(&self, record: &CostRecord) -> Result<()> {
        if let Some(parent) = self.storage_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.storage_path)?;

        let json = serde_json::to_string(record)?;
        writeln!(file, "{}", json)?;
        Ok(())
    }
}
