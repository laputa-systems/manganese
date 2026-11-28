use crate::database::Database;
use crate::database::RazeIfIncompatible;
use crate::database::StatementId;
use crate::sql_value::SqlValue;

const VERSION_KEY: &[u8] = b"version";
const COMPATIBLE_VERSION_KEY: &[u8] = b"last_compatible_version";
const MMAP_STATUS_KEY: &[u8] = b"mmap_status";
const MMAP_SUCCESS: i64 = -1;
const NO_LOWEST_SUPPORTED_VERSION: i32 = 0;

pub struct MetaTable;

pub enum MetaValue {
    Integer(i64),
    Text(Vec<u8>),
}

fn prepare_set(db: &mut Database, key: &[u8]) -> StatementId {
    let id = db.unique_statement(b"INSERT OR REPLACE INTO meta(key,value) VALUES(?,?)");
    db.bind(id, 0, &SqlValue::Text(key.to_vec()));
    id
}

fn prepare_get(db: &mut Database, key: &[u8]) -> (StatementId, bool) {
    let id = db.unique_statement(b"SELECT value FROM meta WHERE key=?");
    if !db.is_valid(id) {
        return (id, false);
    }
    db.bind(id, 0, &SqlValue::Text(key.to_vec()));
    let found = db.step(id);
    (id, found)
}

fn set(db: &mut Database, key: &[u8], value: SqlValue) -> bool {
    let id = prepare_set(db, key);
    db.bind(id, 1, &value);
    let ok = db.run(id);
    db.finalize(id);
    ok
}

fn get_int64(db: &mut Database, key: &[u8]) -> Option<i64> {
    let (id, found) = prepare_get(db, key);
    let value = found.then(|| db.row(id)[0].int64());
    db.finalize(id);
    value
}

impl MetaTable {
    pub fn does_table_exist(db: &mut Database) -> bool {
        db.does_table_exist(b"meta")
    }

    pub fn init(db: &mut Database, version: i32, compatible: i32) -> bool {
        if !db.begin_transaction() {
            return false;
        }
        let ok = Self::create_if_missing(db, version, compatible);
        if !ok {
            if db.is_open() {
                db.rollback_transaction();
            }
            return false;
        }
        db.commit_transaction()
    }

    fn create_if_missing(db: &mut Database, version: i32, compatible: i32) -> bool {
        if Self::does_table_exist(db) {
            return true;
        }
        db.execute("CREATE TABLE meta(key LONGVARCHAR NOT NULL UNIQUE PRIMARY KEY, value LONGVARCHAR)")
            && set(db, MMAP_STATUS_KEY, SqlValue::Integer(MMAP_SUCCESS))
            && Self::set_version(db, version)
            && Self::set_compatible_version(db, compatible)
    }

    pub fn set_version(db: &mut Database, version: i32) -> bool {
        set(db, VERSION_KEY, SqlValue::Integer(i64::from(version)))
    }

    pub fn version(db: &mut Database) -> i32 {
        get_int64(db, VERSION_KEY).map_or(0, |value| value as i32)
    }

    pub fn set_compatible_version(db: &mut Database, version: i32) -> bool {
        set(db, COMPATIBLE_VERSION_KEY, SqlValue::Integer(i64::from(version)))
    }

    pub fn compatible_version(db: &mut Database) -> i32 {
        get_int64(db, COMPATIBLE_VERSION_KEY).map_or(0, |value| value as i32)
    }

    pub fn set_value(db: &mut Database, key: &[u8], value: MetaValue) -> bool {
        match value {
            MetaValue::Integer(value) => set(db, key, SqlValue::Integer(value)),
            MetaValue::Text(value) => set(db, key, SqlValue::Text(value)),
        }
    }

    pub fn int_value(db: &mut Database, key: &[u8]) -> Option<i64> {
        get_int64(db, key)
    }

    pub fn text_value(db: &mut Database, key: &[u8]) -> Option<Vec<u8>> {
        let (id, found) = prepare_get(db, key);
        let value = found.then(|| db.row(id)[0].text());
        db.finalize(id);
        value
    }

    pub fn delete_key(db: &mut Database, key: &[u8]) -> bool {
        let id = db.unique_statement(b"DELETE FROM meta WHERE key=?");
        db.bind(id, 0, &SqlValue::Text(key.to_vec()));
        let ok = db.run(id);
        db.finalize(id);
        ok
    }

    pub fn raze_if_incompatible(db: &mut Database, lowest_supported: i32, current: i32) -> RazeIfIncompatible {
        if !Self::does_table_exist(db) {
            return RazeIfIncompatible::Compatible;
        }
        let (id, found) = prepare_get(db, VERSION_KEY);
        if !found {
            db.finalize(id);
            return RazeIfIncompatible::Failed;
        }
        let on_disk_version = db.row(id)[0].int64();
        db.finalize(id);
        let (id, found) = prepare_get(db, COMPATIBLE_VERSION_KEY);
        if !found {
            db.finalize(id);
            return RazeIfIncompatible::Failed;
        }
        let on_disk_compatible = db.row(id)[0].int64() as i32;
        db.finalize(id);
        let too_old = lowest_supported != NO_LOWEST_SUPPORTED_VERSION && i64::from(lowest_supported) > on_disk_version;
        if too_old || current < on_disk_compatible {
            return if db.raze() { RazeIfIncompatible::RazedSuccessfully } else { RazeIfIncompatible::Failed };
        }
        RazeIfIncompatible::Compatible
    }
}
