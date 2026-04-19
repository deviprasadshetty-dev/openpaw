use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CronJob {
    pub id: String,
    pub expression: String,
    pub command: String,
    pub paused: bool,
    pub one_shot: bool,
    pub next_run_secs: i64,
    pub last_status: Option<String>,
    pub last_run_secs: Option<i64>,
    pub channel: Option<String>,
    pub chat_id: Option<String>,
    pub session_key: Option<String>,
}

pub struct CronScheduler {
    pub jobs: HashMap<String, CronJob>,
    file_path: PathBuf,
}

impl Default for CronScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl CronScheduler {
    pub fn new() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let mut path = PathBuf::from(home);
        path.push(".openpaw");
        fs::create_dir_all(&path).ok();
        path.push("cron.json");

        let mut scheduler = Self {
            jobs: HashMap::new(),
            file_path: path,
        };
        scheduler.load();
        scheduler
    }

    pub fn load(&mut self) {
        if let Ok(data) = fs::read_to_string(&self.file_path)
            && let Ok(jobs) = serde_json::from_str::<HashMap<String, CronJob>>(&data) {
                self.jobs = jobs;
            }
    }

    pub fn save(&self) {
        if let Ok(data) = serde_json::to_string_pretty(&self.jobs) {
            fs::write(&self.file_path, data).ok();
        }
    }

    pub fn list_jobs(&self) -> Vec<CronJob> {
        let mut jobs: Vec<_> = self.jobs.values().cloned().collect();
        jobs.sort_by(|a, b| a.id.cmp(&b.id));
        jobs
    }

    pub fn add_job(
        &mut self,
        expression: &str,
        command: &str,
        delay: Option<&str>,
        channel: &str,
        chat_id: &str,
        session_key: &str,
    ) -> Result<CronJob, anyhow::Error> {
        use std::time::{SystemTime, UNIX_EPOCH};
        let id = format!(
            "job_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis()
        );
        let one_shot = delay.is_some();
        let expr = delay.unwrap_or(expression).to_string();

        let job = CronJob {
            id: id.clone(),
            expression: expr,
            command: command.to_string(),
            paused: false,
            one_shot,
            next_run_secs: 0,
            last_status: None,
            last_run_secs: None,
            channel: Some(channel.to_string()),
            chat_id: Some(chat_id.to_string()),
            session_key: Some(session_key.to_string()),
        };
        self.jobs.insert(id.clone(), job.clone());
        self.save();
        Ok(job)
    }

    pub fn remove_job(&mut self, id: &str) -> bool {
        let removed = self.jobs.remove(id).is_some();
        if removed {
            self.save();
        }
        removed
    }
}
