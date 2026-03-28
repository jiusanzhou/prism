use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Status of an async task.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Processing {
        progress: Option<f32>,
    },
    Completed {
        result_url: Option<String>,
        result: Option<serde_json::Value>,
    },
    Failed {
        error: String,
    },
    Cancelled,
}

/// A tracked async task entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskEntry {
    pub task_id: String,
    pub upstream_task_id: String,
    pub provider: String,
    pub variant: String,
    pub model: String,
    pub client_key: Option<String>,
    pub status: TaskStatus,
    pub created_at: u64,
    pub updated_at: u64,
    pub callback_url: Option<String>,
    pub upstream_base_url: String,
    pub upstream_status_path: Option<String>,
}

/// In-memory async task manager with dual-index lookup.
pub struct AsyncTaskManager {
    tasks: DashMap<String, TaskEntry>,
    upstream_index: DashMap<String, String>,
}

impl AsyncTaskManager {
    pub fn new() -> Self {
        Self {
            tasks: DashMap::new(),
            upstream_index: DashMap::new(),
        }
    }

    /// Generate a unique Prism task ID.
    fn generate_task_id(&self) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let nanos = now.as_nanos();
        let count = self.tasks.len() as u128;
        let mut hasher = Sha256::new();
        hasher.update(nanos.to_le_bytes());
        hasher.update(count.to_le_bytes());
        // Add some entropy from the address of self
        hasher.update((self as *const Self as usize).to_le_bytes());
        let hash = hasher.finalize();
        format!("ptask_{}", &hex::encode(&hash[..12]))
    }

    /// Store a new task entry, return the Prism task ID.
    pub fn submit(&self, mut entry: TaskEntry) -> String {
        let task_id = if entry.task_id.is_empty() {
            self.generate_task_id()
        } else {
            entry.task_id.clone()
        };
        entry.task_id = task_id.clone();
        self.upstream_index
            .insert(entry.upstream_task_id.clone(), task_id.clone());
        self.tasks.insert(task_id.clone(), entry);
        task_id
    }

    /// Lookup by Prism task ID.
    pub fn get(&self, task_id: &str) -> Option<TaskEntry> {
        self.tasks.get(task_id).map(|e| e.clone())
    }

    /// Lookup by upstream (provider) task ID.
    pub fn get_by_upstream(&self, upstream_id: &str) -> Option<TaskEntry> {
        let prism_id = self.upstream_index.get(upstream_id)?;
        self.tasks.get(prism_id.value()).map(|e| e.clone())
    }

    /// Update status for a task.
    pub fn update_status(&self, task_id: &str, status: TaskStatus) {
        if let Some(mut entry) = self.tasks.get_mut(task_id) {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            entry.status = status;
            entry.updated_at = now;
        }
    }

    /// List tasks with optional filters.
    pub fn list(
        &self,
        client_key: Option<&str>,
        status_filter: Option<&str>,
        limit: usize,
    ) -> Vec<TaskEntry> {
        let mut results: Vec<TaskEntry> = self
            .tasks
            .iter()
            .filter(|entry| {
                if let Some(key) = client_key {
                    if entry.client_key.as_deref() != Some(key) {
                        return false;
                    }
                }
                if let Some(sf) = status_filter {
                    let matches = match (&entry.status, sf) {
                        (TaskStatus::Pending, "pending") => true,
                        (TaskStatus::Processing { .. }, "processing") => true,
                        (TaskStatus::Completed { .. }, "completed") => true,
                        (TaskStatus::Failed { .. }, "failed") => true,
                        (TaskStatus::Cancelled, "cancelled") => true,
                        _ => false,
                    };
                    if !matches {
                        return false;
                    }
                }
                true
            })
            .map(|entry| entry.clone())
            .collect();
        // Sort by created_at descending (newest first)
        results.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        results.truncate(limit);
        results
    }

    /// Mark a task as cancelled.
    pub fn cancel(&self, task_id: &str) -> Result<(), String> {
        match self.tasks.get_mut(task_id) {
            Some(mut entry) => {
                match &entry.status {
                    TaskStatus::Completed { .. } => {
                        return Err("cannot cancel completed task".to_string());
                    }
                    TaskStatus::Failed { .. } => {
                        return Err("cannot cancel failed task".to_string());
                    }
                    TaskStatus::Cancelled => {
                        return Err("task already cancelled".to_string());
                    }
                    _ => {}
                }
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                entry.status = TaskStatus::Cancelled;
                entry.updated_at = now;
                Ok(())
            }
            None => Err("task not found".to_string()),
        }
    }

    /// Purge tasks older than `max_age_hours` that are completed, failed, or cancelled.
    /// Returns the number of removed tasks.
    pub fn cleanup(&self, max_age_hours: u64) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let cutoff = now.saturating_sub(max_age_hours * 3600);

        let mut to_remove = Vec::new();
        for entry in self.tasks.iter() {
            let dominated = matches!(
                entry.status,
                TaskStatus::Completed { .. } | TaskStatus::Failed { .. } | TaskStatus::Cancelled
            );
            if dominated && entry.updated_at < cutoff {
                to_remove.push((entry.task_id.clone(), entry.upstream_task_id.clone()));
            }
        }

        let count = to_remove.len() as u64;
        for (task_id, upstream_id) in to_remove {
            self.tasks.remove(&task_id);
            self.upstream_index.remove(&upstream_id);
        }
        count
    }

    /// Spawn a background Tokio task that runs cleanup at the given interval.
    pub fn spawn_cleanup_task(self: &Arc<Self>, interval_hours: u64, max_age_hours: u64) {
        let mgr = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(interval_hours * 3600));
            loop {
                interval.tick().await;
                let removed = mgr.cleanup(max_age_hours);
                if removed > 0 {
                    tracing::info!(removed, "async task cleanup completed");
                }
            }
        });
    }
}

impl Default for AsyncTaskManager {
    fn default() -> Self {
        Self::new()
    }
}

// ──────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn make_entry(upstream_id: &str, provider: &str) -> TaskEntry {
        let now = now_secs();
        TaskEntry {
            task_id: String::new(),
            upstream_task_id: upstream_id.to_string(),
            provider: provider.to_string(),
            variant: format!("model@{provider}"),
            model: "sora-v1".to_string(),
            client_key: Some("client-1".to_string()),
            status: TaskStatus::Pending,
            created_at: now,
            updated_at: now,
            callback_url: None,
            upstream_base_url: "https://api.example.com".to_string(),
            upstream_status_path: Some("/v1/tasks/{id}/status".to_string()),
        }
    }

    #[test]
    fn test_submit_and_get() {
        let mgr = AsyncTaskManager::new();
        let entry = make_entry("up-123", "sora");
        let task_id = mgr.submit(entry);

        assert!(task_id.starts_with("ptask_"));
        let fetched = mgr.get(&task_id).unwrap();
        assert_eq!(fetched.upstream_task_id, "up-123");
        assert_eq!(fetched.provider, "sora");
        assert_eq!(fetched.status, TaskStatus::Pending);
    }

    #[test]
    fn test_get_by_upstream() {
        let mgr = AsyncTaskManager::new();
        let entry = make_entry("up-456", "runway");
        let task_id = mgr.submit(entry);

        let fetched = mgr.get_by_upstream("up-456").unwrap();
        assert_eq!(fetched.task_id, task_id);
        assert_eq!(fetched.provider, "runway");
    }

    #[test]
    fn test_get_nonexistent() {
        let mgr = AsyncTaskManager::new();
        assert!(mgr.get("nonexistent").is_none());
        assert!(mgr.get_by_upstream("nonexistent").is_none());
    }

    #[test]
    fn test_update_status() {
        let mgr = AsyncTaskManager::new();
        let entry = make_entry("up-789", "kling");
        let task_id = mgr.submit(entry);

        mgr.update_status(
            &task_id,
            TaskStatus::Processing {
                progress: Some(0.5),
            },
        );
        let fetched = mgr.get(&task_id).unwrap();
        assert_eq!(
            fetched.status,
            TaskStatus::Processing {
                progress: Some(0.5)
            }
        );

        mgr.update_status(
            &task_id,
            TaskStatus::Completed {
                result_url: Some("https://cdn.example.com/video.mp4".to_string()),
                result: None,
            },
        );
        let fetched = mgr.get(&task_id).unwrap();
        match &fetched.status {
            TaskStatus::Completed { result_url, .. } => {
                assert_eq!(
                    result_url.as_deref(),
                    Some("https://cdn.example.com/video.mp4")
                );
            }
            _ => panic!("expected Completed"),
        }
    }

    #[test]
    fn test_status_transitions() {
        let mgr = AsyncTaskManager::new();
        let entry = make_entry("up-trans", "sora");
        let task_id = mgr.submit(entry);

        // Pending → Processing
        mgr.update_status(&task_id, TaskStatus::Processing { progress: None });
        assert!(matches!(
            mgr.get(&task_id).unwrap().status,
            TaskStatus::Processing { .. }
        ));

        // Processing → Failed
        mgr.update_status(
            &task_id,
            TaskStatus::Failed {
                error: "timeout".to_string(),
            },
        );
        assert!(matches!(
            mgr.get(&task_id).unwrap().status,
            TaskStatus::Failed { .. }
        ));
    }

    #[test]
    fn test_list_all() {
        let mgr = AsyncTaskManager::new();
        mgr.submit(make_entry("up-a", "sora"));
        mgr.submit(make_entry("up-b", "runway"));
        mgr.submit(make_entry("up-c", "kling"));

        let all = mgr.list(None, None, 100);
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn test_list_by_client_key() {
        let mgr = AsyncTaskManager::new();
        mgr.submit(make_entry("up-a", "sora"));

        let mut entry_b = make_entry("up-b", "runway");
        entry_b.client_key = Some("client-2".to_string());
        mgr.submit(entry_b);

        let filtered = mgr.list(Some("client-1"), None, 100);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].provider, "sora");
    }

    #[test]
    fn test_list_by_status() {
        let mgr = AsyncTaskManager::new();
        let id1 = mgr.submit(make_entry("up-a", "sora"));
        mgr.submit(make_entry("up-b", "runway"));

        mgr.update_status(
            &id1,
            TaskStatus::Completed {
                result_url: None,
                result: None,
            },
        );

        let completed = mgr.list(None, Some("completed"), 100);
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].task_id, id1);

        let pending = mgr.list(None, Some("pending"), 100);
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn test_list_with_limit() {
        let mgr = AsyncTaskManager::new();
        for i in 0..10 {
            mgr.submit(make_entry(&format!("up-{i}"), "sora"));
        }
        let limited = mgr.list(None, None, 3);
        assert_eq!(limited.len(), 3);
    }

    #[test]
    fn test_cancel_pending() {
        let mgr = AsyncTaskManager::new();
        let task_id = mgr.submit(make_entry("up-cancel", "sora"));

        assert!(mgr.cancel(&task_id).is_ok());
        assert_eq!(mgr.get(&task_id).unwrap().status, TaskStatus::Cancelled);
    }

    #[test]
    fn test_cancel_processing() {
        let mgr = AsyncTaskManager::new();
        let task_id = mgr.submit(make_entry("up-cancel2", "sora"));
        mgr.update_status(&task_id, TaskStatus::Processing { progress: None });

        assert!(mgr.cancel(&task_id).is_ok());
        assert_eq!(mgr.get(&task_id).unwrap().status, TaskStatus::Cancelled);
    }

    #[test]
    fn test_cancel_completed_fails() {
        let mgr = AsyncTaskManager::new();
        let task_id = mgr.submit(make_entry("up-done", "sora"));
        mgr.update_status(
            &task_id,
            TaskStatus::Completed {
                result_url: None,
                result: None,
            },
        );

        let err = mgr.cancel(&task_id).unwrap_err();
        assert!(err.contains("cannot cancel completed"));
    }

    #[test]
    fn test_cancel_already_cancelled() {
        let mgr = AsyncTaskManager::new();
        let task_id = mgr.submit(make_entry("up-dup", "sora"));
        mgr.cancel(&task_id).unwrap();

        let err = mgr.cancel(&task_id).unwrap_err();
        assert!(err.contains("already cancelled"));
    }

    #[test]
    fn test_cancel_nonexistent() {
        let mgr = AsyncTaskManager::new();
        let err = mgr.cancel("no-such-task").unwrap_err();
        assert!(err.contains("not found"));
    }

    #[test]
    fn test_cleanup() {
        let mgr = AsyncTaskManager::new();
        let task_id = mgr.submit(make_entry("up-old", "sora"));

        // Mark completed with a very old updated_at
        mgr.update_status(
            &task_id,
            TaskStatus::Completed {
                result_url: None,
                result: None,
            },
        );
        // Manually set updated_at to 48 hours ago
        if let Some(mut entry) = mgr.tasks.get_mut(&task_id) {
            entry.updated_at = now_secs() - 48 * 3600;
        }

        // Also add a fresh pending task — should NOT be cleaned
        mgr.submit(make_entry("up-fresh", "runway"));

        let removed = mgr.cleanup(24);
        assert_eq!(removed, 1);
        assert!(mgr.get(&task_id).is_none());
        assert!(mgr.get_by_upstream("up-old").is_none());
        // Fresh task still exists
        assert_eq!(mgr.list(None, None, 100).len(), 1);
    }

    #[test]
    fn test_cleanup_keeps_recent() {
        let mgr = AsyncTaskManager::new();
        let task_id = mgr.submit(make_entry("up-recent", "sora"));
        mgr.update_status(
            &task_id,
            TaskStatus::Completed {
                result_url: None,
                result: None,
            },
        );

        let removed = mgr.cleanup(24);
        assert_eq!(removed, 0);
        assert!(mgr.get(&task_id).is_some());
    }

    #[test]
    fn test_cleanup_keeps_active() {
        let mgr = AsyncTaskManager::new();
        let task_id = mgr.submit(make_entry("up-active", "sora"));
        mgr.update_status(
            &task_id,
            TaskStatus::Processing {
                progress: Some(0.5),
            },
        );
        // Even if old, active tasks should not be cleaned
        if let Some(mut entry) = mgr.tasks.get_mut(&task_id) {
            entry.updated_at = now_secs() - 48 * 3600;
        }

        let removed = mgr.cleanup(24);
        assert_eq!(removed, 0);
    }

    #[test]
    fn test_task_serialization() {
        let entry = TaskEntry {
            task_id: "ptask_abc".to_string(),
            upstream_task_id: "up-123".to_string(),
            provider: "sora".to_string(),
            variant: "sora-v1@sora".to_string(),
            model: "sora-v1".to_string(),
            client_key: None,
            status: TaskStatus::Processing {
                progress: Some(0.75),
            },
            created_at: 1700000000,
            updated_at: 1700000100,
            callback_url: None,
            upstream_base_url: "https://api.sora.com".to_string(),
            upstream_status_path: None,
        };

        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["status"]["status"], "processing");
        assert_eq!(json["status"]["progress"], 0.75);

        // Roundtrip
        let back: TaskEntry = serde_json::from_value(json).unwrap();
        assert_eq!(back.task_id, "ptask_abc");
        assert_eq!(
            back.status,
            TaskStatus::Processing {
                progress: Some(0.75)
            }
        );
    }

    #[test]
    fn test_webhook_update_via_upstream_lookup() {
        let mgr = AsyncTaskManager::new();
        let entry = make_entry("webhook-up-id", "runway");
        let task_id = mgr.submit(entry);

        // Simulate webhook: lookup by upstream ID, update status
        let found = mgr.get_by_upstream("webhook-up-id").unwrap();
        mgr.update_status(
            &found.task_id,
            TaskStatus::Completed {
                result_url: Some("https://cdn.runway.com/output.mp4".to_string()),
                result: None,
            },
        );

        let updated = mgr.get(&task_id).unwrap();
        match &updated.status {
            TaskStatus::Completed { result_url, .. } => {
                assert_eq!(
                    result_url.as_deref(),
                    Some("https://cdn.runway.com/output.mp4")
                );
            }
            _ => panic!("expected Completed"),
        }
    }
}
