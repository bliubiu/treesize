//! 扫描历史 SQLite 存储
//!
//! 提供扫描快照的持久化存储与查询。
//! 数据库文件默认位于 `{app_data}/treesize/history.db`。

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};

use crate::application::models::{compute_top_growing, CategoryTrend, SizePoint, TrendReport};
use crate::domain::scan_history::{CategorySnapshot, DirSizeSnapshot, ScanSnapshot};

/// 当前数据库 schema 版本

/// 扫描历史存储
pub struct HistoryStorage {
    conn: Connection,
}

impl HistoryStorage {
    /// 打开（或创建）指定路径的数据库
    pub fn open(db_path: &Path) -> std::result::Result<Self, rusqlite::Error> {
        let conn = Connection::open(db_path)?;
        let storage = Self { conn };
        storage.migrate()?;
        Ok(storage)
    }

    /// 打开默认位置的数据库（用户数据目录）
    pub fn open_default() -> std::result::Result<Self, rusqlite::Error> {
        let db_path = Self::default_db_path();
        // 确保父目录存在
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        Self::open(&db_path)
    }

    /// 获取默认数据库路径
    fn default_db_path() -> PathBuf {
        let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("treesize");
        base.join("history.db")
    }

    // ── schema 迁移 ─────────────────────────────────────────────────────

    /// 执行 schema 迁移
    fn migrate(&self) -> std::result::Result<(), rusqlite::Error> {
        let version = self.schema_version();

        if version < 1 {
            self.conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS scan_snapshots (
                    id              INTEGER PRIMARY KEY AUTOINCREMENT,
                    scanned_path    TEXT    NOT NULL,
                    scanned_at      TEXT    NOT NULL,
                    total_files     INTEGER NOT NULL,
                    total_dirs      INTEGER NOT NULL,
                    total_size      INTEGER NOT NULL,
                    elapsed_ms      INTEGER NOT NULL DEFAULT 0
                );

                CREATE TABLE IF NOT EXISTS category_snapshots (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    scan_id     INTEGER NOT NULL REFERENCES scan_snapshots(id) ON DELETE CASCADE,
                    category    TEXT    NOT NULL,
                    size        INTEGER NOT NULL,
                    file_count  INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS dir_snapshots (
                    id              INTEGER PRIMARY KEY AUTOINCREMENT,
                    scan_id         INTEGER NOT NULL REFERENCES scan_snapshots(id) ON DELETE CASCADE,
                    relative_path   TEXT    NOT NULL,
                    size            INTEGER NOT NULL
                );

                CREATE INDEX IF NOT EXISTS idx_snapshots_path
                    ON scan_snapshots(scanned_path);

                CREATE INDEX IF NOT EXISTS idx_category_scan
                    ON category_snapshots(scan_id);

                CREATE INDEX IF NOT EXISTS idx_dir_scan
                    ON dir_snapshots(scan_id);

                PRAGMA user_version = 1;
                ",
            )?;
        }

        Ok(())
    }

    /// 读取当前 schema 版本（PRAGMA user_version）
    fn schema_version(&self) -> i64 {
        self.conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap_or(0)
    }

    // ── 写入 ─────────────────────────────────────────────────────────────

    /// 保存一次扫描快照（含分类和目录大小明细）
    pub fn save_snapshot(
        &self,
        snapshot: &ScanSnapshot,
        categories: &[CategorySnapshot],
        dirs: &[DirSizeSnapshot],
    ) -> std::result::Result<i64, rusqlite::Error> {
        let tx = self.conn.unchecked_transaction()?;

        tx.execute(
            "INSERT INTO scan_snapshots (scanned_path, scanned_at, total_files, total_dirs, total_size, elapsed_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                snapshot.scanned_path,
                snapshot.scanned_at.format("%Y-%m-%dT%H:%M:%S").to_string(),
                snapshot.total_files,
                snapshot.total_dirs,
                snapshot.total_size,
                snapshot.elapsed_ms,
            ],
        )?;

        let scan_id = tx.last_insert_rowid();

        for cat in categories {
            tx.execute(
                "INSERT INTO category_snapshots (scan_id, category, size, file_count) VALUES (?1, ?2, ?3, ?4)",
                params![scan_id, cat.category, cat.size, cat.file_count],
            )?;
        }

        for dir in dirs {
            tx.execute(
                "INSERT INTO dir_snapshots (scan_id, relative_path, size) VALUES (?1, ?2, ?3)",
                params![scan_id, dir.relative_path, dir.size],
            )?;
        }

        tx.commit()?;
        Ok(scan_id)
    }

    // ── 读取 ───────────────────────────────────────────────────────────────

    /// 获取指定路径的所有扫描快照（按时间升序）
    pub fn list_snapshots(&self, scan_path: &str) -> std::result::Result<Vec<ScanSnapshot>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT id, scanned_path, scanned_at, total_files, total_dirs, total_size, elapsed_ms
             FROM scan_snapshots
             WHERE scanned_path = ?1
             ORDER BY scanned_at ASC",
        )?;

        let snapshots = stmt
            .query_map(params![scan_path], |row| Self::parse_snapshot_row(row))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(snapshots)
    }

    /// 获取某次快照的分类明细
    pub fn get_categories(&self, scan_id: i64) -> std::result::Result<Vec<CategorySnapshot>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT category, size, file_count FROM category_snapshots WHERE scan_id = ?1 ORDER BY size DESC",
        )?;

        let cats = stmt
            .query_map(params![scan_id], |row| {
                Ok(CategorySnapshot {
                    category: row.get(0)?,
                    size: row.get(1)?,
                    file_count: row.get(2)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(cats)
    }

    /// 获取某次快照的目录大小明细
    pub fn get_dirs(&self, scan_id: i64) -> std::result::Result<Vec<DirSizeSnapshot>, rusqlite::Error> {
        let mut stmt = self
            .conn
            .prepare("SELECT relative_path, size FROM dir_snapshots WHERE scan_id = ?1 ORDER BY size DESC")?;

        let dirs = stmt
            .query_map(params![scan_id], |row| {
                Ok(DirSizeSnapshot {
                    relative_path: row.get(0)?,
                    size: row.get(1)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(dirs)
    }

    /// 获取完整趋势报告
    pub fn get_trend(&self, scan_path: &str) -> std::result::Result<TrendReport, rusqlite::Error> {
        let snapshots = self.list_snapshots(scan_path)?;
        if snapshots.is_empty() {
            return Ok(TrendReport {
                path: scan_path.to_string(),
                snapshots: vec![],
                size_trend: vec![],
                top_growing: vec![],
                category_trends: vec![],
            });
        }

        // 总大小趋势
        let size_trend: Vec<SizePoint> = snapshots
            .iter()
            .map(|s| SizePoint {
                date: s.scanned_at.date_naive(),
                total_size: s.total_size,
            })
            .collect();

        // 类别趋势（所有快照的所有分类）
        let mut category_trends: Vec<CategoryTrend> = Vec::new();
        for snap in &snapshots {
            if let Some(id) = snap.id {
                if let Ok(cats) = self.get_categories(id) {
                    for cat in cats {
                        let trend = category_trends
                            .iter_mut()
                            .find(|t: &&mut CategoryTrend| t.category == cat.category);
                        if let Some(t) = trend {
                            t.size_history.push(SizePoint {
                                date: snap.scanned_at.date_naive(),
                                total_size: cat.size,
                            });
                        } else {
                            category_trends.push(CategoryTrend {
                                category: cat.category,
                                size_history: vec![SizePoint {
                                    date: snap.scanned_at.date_naive(),
                                    total_size: cat.size,
                                }],
                            });
                        }
                    }
                }
            }
        }

        // 收集各次快照的目录大小明细（用于增长排名）
        let mut dirs_by_scan: Vec<Vec<DirSizeSnapshot>> = Vec::new();
        for snap in &snapshots {
            if let Some(id) = snap.id {
                dirs_by_scan.push(self.get_dirs(id).unwrap_or_default());
            } else {
                dirs_by_scan.push(vec![]);
            }
        }

        // 增长最快的顶层子目录（Top 10）
        let top_growing = compute_top_growing(&snapshots, &dirs_by_scan);

        Ok(TrendReport {
            path: scan_path.to_string(),
            snapshots,
            size_trend,
            top_growing,
            category_trends,
        })
    }

    /// 删除某次快照
    pub fn delete_snapshot(&self, scan_id: i64) -> std::result::Result<(), rusqlite::Error> {
        // CASCADE 会级联删除 category_snapshots 和 dir_snapshots
        self.conn
            .execute("DELETE FROM scan_snapshots WHERE id = ?1", params![scan_id])?;
        Ok(())
    }

    /// 列出所有有历史记录的扫描路径
    pub fn list_paths(&self) -> std::result::Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT scanned_path FROM scan_snapshots ORDER BY scanned_path")?;

        let paths = stmt
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(paths)
    }

    /// 将 SQL 行解析为 ScanSnapshot
    fn parse_snapshot_row(row: &rusqlite::Row) -> rusqlite::Result<ScanSnapshot> {
        let scanned_at_str: String = row.get(2)?;
        let scanned_at = chrono::NaiveDateTime::parse_from_str(&scanned_at_str, "%Y-%m-%dT%H:%M:%S")
            .map(|naive| naive.and_local_timezone(chrono::Local).unwrap())
            .unwrap_or_else(|_| chrono::Local::now());

        Ok(ScanSnapshot {
            id: Some(row.get(0)?),
            scanned_path: row.get(1)?,
            scanned_at,
            total_files: row.get(3)?,
            total_dirs: row.get(4)?,
            total_size: row.get(5)?,
            elapsed_ms: row.get(6)?,
        })
    }

    /// 获取两次快照的对比数据
    pub fn get_diff_data(
        &self,
        old_id: i64,
        new_id: i64,
    ) -> std::result::Result<
        (
            ScanSnapshot,
            ScanSnapshot,
            Vec<CategorySnapshot>,
            Vec<CategorySnapshot>,
            Vec<DirSizeSnapshot>,
            Vec<DirSizeSnapshot>,
        ),
        rusqlite::Error,
    > {
        let mut stmt = self.conn.prepare(
            "SELECT id, scanned_path, scanned_at, total_files, total_dirs, total_size, elapsed_ms
             FROM scan_snapshots WHERE id = ?1",
        )?;

        let old = stmt.query_row(params![old_id], |row| Self::parse_snapshot_row(row))?;
        let new = stmt.query_row(params![new_id], |row| Self::parse_snapshot_row(row))?;

        let old_cats = self.get_categories(old_id)?;
        let new_cats = self.get_categories(new_id)?;
        let old_dirs = self.get_dirs(old_id)?;
        let new_dirs = self.get_dirs(new_id)?;

        Ok((old, new, old_cats, new_cats, old_dirs, new_dirs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::scan_history::{CategorySnapshot, DirSizeSnapshot, ScanSnapshot};
    use tempfile::tempdir;

    fn make_snapshot(path: &str, files: u64, dirs: u64, size: u64, elapsed: u64) -> ScanSnapshot {
        ScanSnapshot {
            id: None,
            scanned_path: path.to_string(),
            scanned_at: chrono::Local::now(),
            total_files: files,
            total_dirs: dirs,
            total_size: size,
            elapsed_ms: elapsed,
        }
    }

    fn open_temp_db() -> (HistoryStorage, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let storage = HistoryStorage::open(&db_path).unwrap();
        (storage, dir)
    }

    // ── Schema 迁移 ─────────────────────────────────────────────────────

    #[test]
    fn migrate_creates_tables() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let storage = HistoryStorage::open(&db_path).unwrap();

        // 验证表已创建：通过查询来验证
        let tables: Vec<String> = storage
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();

        assert!(
            tables.contains(&"scan_snapshots".to_string()),
            "应包含 scan_snapshots 表"
        );
        assert!(
            tables.contains(&"category_snapshots".to_string()),
            "应包含 category_snapshots 表"
        );
        assert!(tables.contains(&"dir_snapshots".to_string()), "应包含 dir_snapshots 表");
    }

    #[test]
    fn migrate_idempotent() {
        // 验证重复 migrate 不会报错
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let storage = HistoryStorage::open(&db_path).unwrap();
        // 再次调用 migrate
        storage.migrate().unwrap();
        // 不崩溃即通过
    }

    // ── CRUD 操作 ──────────────────────────────────────────────────────

    #[test]
    fn save_and_list_snapshots() {
        let (storage, _dir) = open_temp_db();
        let snap = make_snapshot("/test", 100, 10, 1_000_000, 500);

        let scan_id = storage.save_snapshot(&snap, &[], &[]).expect("保存快照应成功");
        assert!(scan_id > 0, "scan_id 应为正数");

        let list = storage.list_snapshots("/test").expect("列出快照应成功");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].scanned_path, "/test");
        assert_eq!(list[0].total_files, 100);
        assert_eq!(list[0].total_dirs, 10);
        assert_eq!(list[0].total_size, 1_000_000);
        assert_eq!(list[0].elapsed_ms, 500);
        assert!(list[0].id.is_some(), "读出后 id 不应为 None");
    }

    #[test]
    fn list_snapshots_returns_empty_for_non_existent_path() {
        let (storage, _dir) = open_temp_db();
        let list = storage.list_snapshots("/nonexistent").unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn save_with_categories_and_dirs() {
        let (storage, _dir) = open_temp_db();
        let snap = make_snapshot("/test", 50, 5, 500_000, 300);

        let cats = vec![
            CategorySnapshot {
                category: "文档".to_string(),
                size: 300_000,
                file_count: 30,
            },
            CategorySnapshot {
                category: "视频".to_string(),
                size: 200_000,
                file_count: 5,
            },
        ];
        let dirs_data = vec![
            DirSizeSnapshot {
                relative_path: "sub1".to_string(),
                size: 300_000,
            },
            DirSizeSnapshot {
                relative_path: "sub2".to_string(),
                size: 200_000,
            },
        ];

        let scan_id = storage
            .save_snapshot(&snap, &cats, &dirs_data)
            .expect("保存含明细的快照应成功");

        // 验证分类
        let saved_cats = storage.get_categories(scan_id).unwrap();
        assert_eq!(saved_cats.len(), 2);
        let doc = saved_cats.iter().find(|c| c.category == "文档").unwrap();
        assert_eq!(doc.size, 300_000);
        assert_eq!(doc.file_count, 30);

        // 验证目录
        let saved_dirs = storage.get_dirs(scan_id).unwrap();
        assert_eq!(saved_dirs.len(), 2);
        let sub1 = saved_dirs.iter().find(|d| d.relative_path == "sub1").unwrap();
        assert_eq!(sub1.size, 300_000);
    }

    #[test]
    fn multiple_snapshots_same_path() {
        let (storage, _dir) = open_temp_db();

        let snap1 = make_snapshot("/test", 50, 5, 500_000, 300);
        let snap2 = make_snapshot("/test", 100, 10, 1_000_000, 500);

        storage.save_snapshot(&snap1, &[], &[]).unwrap();
        storage.save_snapshot(&snap2, &[], &[]).unwrap();

        let list = storage.list_snapshots("/test").unwrap();
        assert_eq!(list.len(), 2);
        // 按时间升序，第一条应是 snap1
        assert_eq!(list[0].total_files, 50);
        assert_eq!(list[1].total_files, 100);
    }

    #[test]
    fn delete_snapshot_cascades() {
        let (storage, _dir) = open_temp_db();
        let snap = make_snapshot("/test", 10, 1, 1000, 100);
        let cats = vec![CategorySnapshot {
            category: "文档".to_string(),
            size: 1000,
            file_count: 10,
        }];

        let scan_id = storage.save_snapshot(&snap, &cats, &[]).unwrap();

        // 删除前分类应存在
        assert_eq!(storage.get_categories(scan_id).unwrap().len(), 1);

        // 删除
        storage.delete_snapshot(scan_id).unwrap();

        // 快照和分类都应被级联删除
        assert!(storage.list_snapshots("/test").unwrap().is_empty());
        assert!(storage.get_categories(scan_id).unwrap().is_empty());
    }

    // ── 路径查询 ───────────────────────────────────────────────────────

    #[test]
    fn list_paths_returns_distinct_paths() {
        let (storage, _dir) = open_temp_db();

        let snap_a = make_snapshot("/path/a", 10, 1, 1000, 100);
        let snap_b = make_snapshot("/path/b", 20, 2, 2000, 200);
        let snap_a2 = make_snapshot("/path/a", 15, 1, 1500, 150);

        storage.save_snapshot(&snap_a, &[], &[]).unwrap();
        storage.save_snapshot(&snap_b, &[], &[]).unwrap();
        storage.save_snapshot(&snap_a2, &[], &[]).unwrap();

        let mut paths = storage.list_paths().unwrap();
        paths.sort();
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0], "/path/a");
        assert_eq!(paths[1], "/path/b");
    }

    // ── 趋势查询 ───────────────────────────────────────────────────────

    #[test]
    fn get_trend_empty_path() {
        let (storage, _dir) = open_temp_db();
        let trend = storage.get_trend("/nonexistent").unwrap();
        assert_eq!(trend.path, "/nonexistent");
        assert!(trend.snapshots.is_empty());
        assert!(trend.size_trend.is_empty());
    }

    #[test]
    fn get_trend_with_data() {
        let (storage, _dir) = open_temp_db();

        let snap1 = make_snapshot("/test", 50, 5, 500_000, 300);
        let cats1 = vec![CategorySnapshot {
            category: "文档".to_string(),
            size: 300_000,
            file_count: 30,
        }];
        let dirs1 = vec![DirSizeSnapshot {
            relative_path: "src".to_string(),
            size: 300_000,
        }];

        let snap2 = make_snapshot("/test", 100, 10, 1_000_000, 500);
        let cats2 = vec![
            CategorySnapshot {
                category: "文档".to_string(),
                size: 600_000,
                file_count: 60,
            },
            CategorySnapshot {
                category: "视频".to_string(),
                size: 400_000,
                file_count: 5,
            },
        ];
        let dirs2 = vec![
            DirSizeSnapshot {
                relative_path: "src".to_string(),
                size: 600_000,
            },
            DirSizeSnapshot {
                relative_path: "assets".to_string(),
                size: 400_000,
            },
        ];

        storage.save_snapshot(&snap1, &cats1, &dirs1).unwrap();
        storage.save_snapshot(&snap2, &cats2, &dirs2).unwrap();

        let trend = storage.get_trend("/test").unwrap();

        assert_eq!(trend.path, "/test");
        assert_eq!(trend.snapshots.len(), 2);
        assert_eq!(trend.size_trend.len(), 2);

        // 增长数据
        assert!(!trend.top_growing.is_empty());

        // 分类趋势
        assert_eq!(trend.category_trends.len(), 2);
        let doc_trend = trend
            .category_trends
            .iter()
            .find(|t| t.category == "文档")
            .expect("应有文档分类趋势");
        assert_eq!(doc_trend.size_history.len(), 2);
    }

    #[test]
    fn get_trend_with_multiple_snapshots_same_category() {
        let (storage, _dir) = open_temp_db();

        for i in 0..3 {
            let snap = make_snapshot("/test", 10 * (i + 1), 1, 1000 * (i + 1), 100);
            let cats = vec![CategorySnapshot {
                category: "图片".to_string(),
                size: 500 * (i + 1) as u64,
                file_count: 5 * (i + 1) as u64,
            }];
            storage.save_snapshot(&snap, &cats, &[]).unwrap();
        }

        let trend = storage.get_trend("/test").unwrap();

        let img = trend
            .category_trends
            .iter()
            .find(|t| t.category == "图片")
            .expect("应有图片趋势");
        assert_eq!(img.size_history.len(), 3);
        assert_eq!(img.size_history[0].total_size, 500);
        assert_eq!(img.size_history[1].total_size, 1000);
        assert_eq!(img.size_history[2].total_size, 1500);
    }

    // ── Diff 数据 ──────────────────────────────────────────────────────

    #[test]
    fn get_diff_data_returns_both_snapshots() {
        let (storage, _dir) = open_temp_db();

        let snap1 = make_snapshot("/test", 50, 5, 500_000, 300);
        let snap2 = make_snapshot("/test", 100, 10, 1_000_000, 500);

        let cats1 = vec![CategorySnapshot {
            category: "文档".to_string(),
            size: 500_000,
            file_count: 50,
        }];
        let dirs1 = vec![DirSizeSnapshot {
            relative_path: "sub".to_string(),
            size: 500_000,
        }];

        let id1 = storage.save_snapshot(&snap1, &cats1, &dirs1).unwrap();
        let id2 = storage.save_snapshot(&snap2, &[], &[]).unwrap();

        let (old, new, old_cats, new_cats, old_dirs, new_dirs) = storage.get_diff_data(id1, id2).unwrap();

        assert_eq!(old.total_files, 50);
        assert_eq!(new.total_files, 100);
        assert_eq!(old_cats.len(), 1);
        assert_eq!(new_cats.len(), 0);
        assert_eq!(old_dirs.len(), 1);
        assert_eq!(new_dirs.len(), 0);
    }

    #[test]
    fn get_diff_data_with_dir_snapshots() {
        let (storage, _dir) = open_temp_db();

        let snap1 = make_snapshot("/test", 100, 10, 1_000_000, 500);
        let dirs1 = vec![
            DirSizeSnapshot {
                relative_path: "src".to_string(),
                size: 600_000,
            },
            DirSizeSnapshot {
                relative_path: "docs".to_string(),
                size: 400_000,
            },
        ];
        let id1 = storage.save_snapshot(&snap1, &[], &dirs1).unwrap();

        let snap2 = make_snapshot("/test", 150, 15, 2_000_000, 600);
        let dirs2 = vec![
            DirSizeSnapshot {
                relative_path: "src".to_string(),
                size: 1_200_000,
            },
            DirSizeSnapshot {
                relative_path: "assets".to_string(),
                size: 800_000,
            },
        ];
        let id2 = storage.save_snapshot(&snap2, &[], &dirs2).unwrap();

        let (_old, _new, _old_cats, _new_cats, old_dirs, new_dirs) = storage.get_diff_data(id1, id2).unwrap();

        assert_eq!(old_dirs.len(), 2);
        assert_eq!(new_dirs.len(), 2);

        let src_old = old_dirs.iter().find(|d| d.relative_path == "src").unwrap();
        assert_eq!(src_old.size, 600_000);
    }
}
