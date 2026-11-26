use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use crate::engine;
use crate::engine::Engine;
use crate::engine::EngineStatement;
use crate::sql_value::SqlValue;

#[derive(Clone, Copy, Debug)]
pub struct DatabaseOptions {
    pub wal_mode: bool,
    pub exclusive_locking: bool,
    pub read_only: bool,
    pub mmap_enabled: bool,
    pub enable_triggers: bool,
    pub enable_views: bool,
    pub no_sync: bool,
    pub page_size: u32,
    pub cache_size: i32,
}

impl DatabaseOptions {
    pub fn from_flags(flags: u8, page_size: u32, cache_size: i32) -> Self {
        Self {
            wal_mode: flags & 0x01 != 0,
            exclusive_locking: flags & 0x02 == 0,
            read_only: flags & 0x04 != 0,
            mmap_enabled: flags & 0x08 == 0,
            enable_triggers: flags & 0x10 != 0,
            enable_views: flags & 0x20 != 0,
            no_sync: flags & 0x40 != 0,
            page_size,
            cache_size,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StatementId(u64);

struct PreparedStatement {
    engine: Option<EngineStatement>,
    ran: bool,
    halted: bool,
    rc: i32,
    last_result: Option<i32>,
}

enum Connection {
    Open(Engine),
    Broken(i32),
}

pub struct Database {
    options: DatabaseOptions,
    connection: Option<Connection>,
    poisoned: bool,
    transaction_nesting: i32,
    needs_rollback: bool,
    errcode: i32,
    epoch: u64,
    next_statement: u64,
    statements: HashMap<StatementId, PreparedStatement>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RazeIfIncompatible {
    Compatible = 0,
    RazedSuccessfully = 1,
    Failed = 2,
}

fn is_sql_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

fn skip_blank(sql: &[u8]) -> usize {
    let mut pos = 0;
    loop {
        while pos < sql.len() && (is_sql_space(sql[pos]) || sql[pos] == b';') {
            pos += 1;
        }
        if sql[pos..].starts_with(b"--") {
            while pos < sql.len() && sql[pos] != b'\n' {
                pos += 1;
            }
        } else if sql[pos..].starts_with(b"/*") {
            match sql[pos + 2..].windows(2).position(|pair| pair == b"*/") {
                Some(end) => pos += 2 + end + 2,
                None => pos = sql.len(),
            }
        } else {
            return pos;
        }
    }
}

fn quote_identifier(name: &[u8]) -> Vec<u8> {
    let mut quoted = vec![b'"'];
    for &byte in name {
        quoted.push(byte);
        if byte == b'"' {
            quoted.push(b'"');
        }
    }
    quoted.push(b'"');
    quoted
}

impl Database {
    pub fn new(options: DatabaseOptions) -> Self {
        Self {
            options,
            connection: None,
            poisoned: false,
            transaction_nesting: 0,
            needs_rollback: false,
            errcode: 0,
            epoch: 0,
            next_statement: 0,
            statements: HashMap::new(),
        }
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn is_open(&self) -> bool {
        self.connection.is_some() && !self.poisoned
    }

    pub fn has_connection(&self) -> bool {
        self.connection.is_some()
    }

    pub fn error_code(&self) -> i32 {
        if self.connection.is_none() {
            return engine::SQLITE_ERROR;
        }
        self.errcode
    }

    pub fn open(&mut self, path: &Path) -> bool {
        if self.open_internal(path) {
            return true;
        }
        if self.poisoned {
            self.close();
            return self.open_internal(path);
        }
        false
    }

    fn open_internal(&mut self, path: &Path) -> bool {
        let path = path.to_str().expect("UTF-8 database path");
        match Engine::open(path, self.options.read_only, self.options.enable_triggers, self.options.enable_views) {
            Ok(engine) => {
                self.connection = Some(Connection::Open(engine));
                self.errcode = 0;
            }
            Err(code) => {
                self.connection = Some(Connection::Broken(code));
                self.errcode = code;
                return false;
            }
        }
        if !self.options.exclusive_locking && !self.execute("PRAGMA locking_mode=NORMAL") {
            return false;
        }
        if !self.options.read_only {
            if !self.execute(&format!("PRAGMA page_size={}", self.options.page_size)) {
                return false;
            }
            if self.options.no_sync || self.options.wal_mode {
                let sql = if self.options.no_sync { "PRAGMA synchronous=OFF" } else { "PRAGMA synchronous=NORMAL" };
                if !self.execute(sql) {
                    return false;
                }
            }
            let journal = if self.options.wal_mode { "PRAGMA journal_mode=WAL" } else { "PRAGMA journal_mode=TRUNCATE" };
            if !self.execute(journal) {
                return false;
            }
            if self.options.cache_size != 0 {
                let _ = self.execute(&format!("PRAGMA cache_size={}", self.options.cache_size));
            }
        }
        let mmap_size = if self.options.mmap_enabled && !self.poisoned { 256 * 1024 * 1024 } else { 0 };
        let _ = self.execute(&format!("PRAGMA mmap_size={mmap_size}"));
        let id = self.unique_statement(b"PRAGMA mmap_size");
        let _ = self.step(id);
        self.finalize(id);
        self.is_open()
    }

    fn close_internal(&mut self) {
        let ids: Vec<StatementId> = self.statements.keys().copied().collect();
        for id in ids {
            if let Some(statement) = self.statements.get_mut(&id) {
                statement.engine = None;
            }
        }
        if let Some(connection) = self.connection.take() {
            self.epoch += 1;
            if let Connection::Open(engine) = connection {
                engine.close();
            }
            self.transaction_nesting = 0;
            self.needs_rollback = false;
        }
    }

    pub fn close(&mut self) {
        if self.poisoned {
            self.poisoned = false;
            return;
        }
        self.close_internal();
    }

    pub fn poison(&mut self) {
        if self.connection.is_none() {
            return;
        }
        self.close_internal();
        self.poisoned = true;
    }

    fn engine(&self) -> Result<&Engine, i32> {
        match &self.connection {
            Some(Connection::Open(engine)) => Ok(engine),
            Some(Connection::Broken(code)) => Err(*code),
            None => Err(engine::SQLITE_ERROR),
        }
    }

    fn prepare_one(&mut self, sql: &str) -> Result<Option<(EngineStatement, usize)>, i32> {
        let start = skip_blank(sql.as_bytes());
        if start == sql.len() {
            self.errcode = 0;
            return Ok(None);
        }
        let result = self.engine().and_then(|engine| engine.prepare(&sql[start..]));
        match result {
            Ok((statement, tail)) => {
                self.errcode = 0;
                Ok(Some((statement, start + tail)))
            }
            Err(code) => {
                self.errcode = code;
                Err(code)
            }
        }
    }

    pub fn execute(&mut self, sql: &str) -> bool {
        if self.connection.is_none() {
            return false;
        }
        let mut rest = sql;
        loop {
            let prepared = match self.prepare_one(rest) {
                Ok(Some(prepared)) => prepared,
                Ok(None) => return true,
                Err(_) => return false,
            };
            let (mut statement, tail) = prepared;
            let engine = self.engine().expect("prepared on an open connection");
            let result = loop {
                match engine.step(&mut statement) {
                    Ok(true) => {}
                    Ok(false) => break Ok(()),
                    Err(code) => break Err(code),
                }
            };
            engine.reset(&mut statement);
            match result {
                Ok(()) => self.errcode = 0,
                Err(code) => {
                    self.errcode = code;
                    return false;
                }
            }
            rest = &rest[tail..];
        }
    }

    pub fn unique_statement(&mut self, sql: &[u8]) -> StatementId {
        let id = StatementId(self.next_statement);
        self.next_statement += 1;
        let engine = if self.connection.is_none() {
            None
        } else {
            let text = String::from_utf8_lossy(sql).into_owned();
            match self.prepare_one(&text) {
                Ok(Some((statement, _))) => Some(statement),
                Ok(None) | Err(_) => None,
            }
        };
        self.statements.insert(id, PreparedStatement { engine, ran: false, halted: false, rc: 0, last_result: None });
        id
    }

    pub fn is_valid(&self, id: StatementId) -> bool {
        self.statements.get(&id).is_some_and(|statement| statement.engine.is_some())
    }

    pub fn bind(&mut self, id: StatementId, index: usize, value: &SqlValue) {
        let Some(Connection::Open(engine)) = &self.connection else { return };
        let Some(statement) = self.statements.get_mut(&id) else { return };
        let Some(prepared) = statement.engine.as_mut() else { return };
        if statement.ran {
            self.errcode = engine::SQLITE_MISUSE;
            return;
        }
        engine.bind(prepared, index + 1, value);
    }

    pub fn step(&mut self, id: StatementId) -> bool {
        self.step_internal(id) == engine::SQLITE_ROW
    }

    pub fn run(&mut self, id: StatementId) -> bool {
        self.step_internal(id) == engine::SQLITE_DONE
    }

    fn step_internal(&mut self, id: StatementId) -> i32 {
        let statement = self.statements.get_mut(&id).expect("known statement");
        let Some(prepared) = statement.engine.as_mut() else {
            statement.last_result = Some(engine::SQLITE_ERROR);
            return engine::SQLITE_ERROR;
        };
        let engine = match &self.connection {
            Some(Connection::Open(engine)) => engine,
            _ => unreachable!("a valid statement has an open connection"),
        };
        if statement.halted {
            engine.reset(prepared);
            statement.halted = false;
            statement.rc = 0;
        }
        statement.ran = true;
        let code = match engine.step(prepared) {
            Ok(true) => engine::SQLITE_ROW,
            Ok(false) => {
                statement.halted = true;
                engine::SQLITE_DONE
            }
            Err(code) => {
                engine.reset(prepared);
                statement.halted = true;
                statement.rc = code;
                code
            }
        };
        statement.last_result = Some(code);
        self.errcode = code;
        code
    }

    pub fn succeeded(&self, id: StatementId) -> bool {
        let Some(statement) = self.statements.get(&id) else { return false };
        statement.engine.is_some()
            && matches!(statement.last_result, Some(0 | engine::SQLITE_ROW | engine::SQLITE_DONE))
    }

    pub fn row(&self, id: StatementId) -> &[SqlValue] {
        let statement = self.statements.get(&id).expect("known statement");
        statement.engine.as_ref().map_or(&[], |prepared| prepared.row.as_slice())
    }

    pub fn reset(&mut self, id: StatementId, clear_bindings: bool) {
        let connection = &self.connection;
        let Some(statement) = self.statements.get_mut(&id) else { return };
        if let (Some(prepared), Some(Connection::Open(engine))) = (statement.engine.as_mut(), connection) {
            if self.transaction_nesting > 0 && statement.ran && !statement.halted {
                while let Ok(true) = engine.step(prepared) {}
            }
            engine.reset(prepared);
            if clear_bindings {
                engine.clear_bindings(prepared);
            }
            if statement.ran {
                self.errcode = statement.rc;
            }
            statement.ran = false;
            statement.halted = false;
            statement.rc = 0;
        }
        statement.last_result = None;
    }

    pub fn finalize(&mut self, id: StatementId) {
        self.reset(id, true);
        self.statements.remove(&id);
    }

    pub fn last_change_count(&self) -> i64 {
        self.engine().map_or(0, Engine::changes)
    }

    pub fn last_insert_row_id(&self) -> i64 {
        self.engine().map_or(0, Engine::last_insert_rowid)
    }

    fn autocommit(&self) -> bool {
        self.engine().map_or(true, Engine::autocommit)
    }

    fn run_sql(&mut self, sql: &str) -> bool {
        let id = self.unique_statement(sql.as_bytes());
        let valid = self.is_valid(id);
        let ok = valid && self.run(id);
        self.finalize(id);
        ok
    }

    pub fn begin_transaction(&mut self) -> bool {
        if self.needs_rollback {
            return false;
        }
        if self.transaction_nesting == 0 {
            self.needs_rollback = false;
            if !self.run_sql("BEGIN TRANSACTION") {
                return false;
            }
        }
        self.transaction_nesting += 1;
        true
    }

    fn do_rollback(&mut self) {
        if self.autocommit() {
            self.needs_rollback = false;
            return;
        }
        let _ = self.run_sql("ROLLBACK");
        self.needs_rollback = false;
    }

    pub fn rollback_transaction(&mut self) {
        if self.transaction_nesting == 0 {
            return;
        }
        self.transaction_nesting -= 1;
        if self.transaction_nesting > 0 {
            self.needs_rollback = true;
            return;
        }
        self.do_rollback();
    }

    pub fn commit_transaction(&mut self) -> bool {
        if self.transaction_nesting == 0 {
            return false;
        }
        self.transaction_nesting -= 1;
        if self.transaction_nesting > 0 {
            return !self.needs_rollback;
        }
        if self.needs_rollback {
            self.do_rollback();
            return false;
        }
        if self.autocommit() {
            return false;
        }
        let succeeded = self.run_sql("COMMIT");
        if !self.is_open() {
            return false;
        }
        if !succeeded && !self.autocommit() {
            self.do_rollback();
            return false;
        }
        succeeded
    }

    pub fn rollback_all_transactions(&mut self) {
        if self.transaction_nesting > 0 {
            self.transaction_nesting = 0;
            self.do_rollback();
        }
    }

    pub fn raze(&mut self) -> bool {
        if self.connection.is_none() || self.transaction_nesting > 0 {
            return false;
        }
        let names = self.query_rows(b"SELECT type, name FROM sqlite_schema");
        let Some(names) = names else { return false };
        let mut drops = Vec::new();
        for row in names {
            let (SqlValue::Text(kind), SqlValue::Text(name)) = (&row[0], &row[1]) else { continue };
            if name.starts_with(b"sqlite_") {
                continue;
            }
            let keyword: &[u8] = match kind.as_slice() {
                b"table" => b"TABLE",
                b"view" => b"VIEW",
                b"trigger" => b"TRIGGER",
                _ => continue,
            };
            let mut sql = b"DROP ".to_vec();
            sql.extend_from_slice(keyword);
            sql.extend_from_slice(b" IF EXISTS ");
            sql.extend_from_slice(&quote_identifier(name));
            drops.push(String::from_utf8_lossy(&sql).into_owned());
        }
        for sql in drops {
            if !self.execute(&sql) {
                if matches!(self.errcode, engine::SQLITE_BUSY | engine::SQLITE_LOCKED) {
                    self.errcode = 0;
                }
                return false;
            }
        }
        self.execute("PRAGMA user_version=0") && self.execute("PRAGMA application_id=0")
    }

    pub fn raze_and_poison(&mut self) -> bool {
        if self.connection.is_none() {
            return false;
        }
        self.rollback_all_transactions();
        let result = self.raze();
        self.close_internal();
        self.poisoned = true;
        result
    }

    pub fn vacuum(&mut self) -> bool {
        if !self.is_open() || self.transaction_nesting != 0 {
            return false;
        }
        self.run_sql("VACUUM")
    }

    pub fn checkpoint(&mut self, truncate: bool) -> bool {
        if self.connection.is_none() {
            return false;
        }
        let sql = if truncate { "PRAGMA wal_checkpoint(TRUNCATE)" } else { "PRAGMA wal_checkpoint(PASSIVE)" };
        self.execute(sql)
    }

    pub fn query_rows(&mut self, sql: &[u8]) -> Option<Vec<Vec<SqlValue>>> {
        let id = self.unique_statement(sql);
        let mut rows = Vec::new();
        while self.step(id) {
            rows.push(self.row(id).to_vec());
        }
        let ok = self.succeeded(id);
        self.finalize(id);
        ok.then_some(rows)
    }

    fn does_schema_item_exist(&mut self, name: &[u8], kind: &[u8]) -> bool {
        let id = self.unique_statement(b"SELECT 1 FROM sqlite_schema WHERE type=? AND name=?");
        if !self.is_valid(id) {
            self.finalize(id);
            return false;
        }
        self.bind(id, 0, &SqlValue::Text(kind.to_vec()));
        self.bind(id, 1, &SqlValue::Text(name.to_vec()));
        let exists = self.step(id);
        self.finalize(id);
        exists
    }

    pub fn does_table_exist(&mut self, name: &[u8]) -> bool {
        self.does_schema_item_exist(name, b"table")
    }

    pub fn does_index_exist(&mut self, name: &[u8]) -> bool {
        self.does_schema_item_exist(name, b"index")
    }

    pub fn does_view_exist(&mut self, name: &[u8]) -> bool {
        self.does_schema_item_exist(name, b"view")
    }

    pub fn does_column_exist(&mut self, table: &[u8], column: &[u8]) -> bool {
        if self.connection.is_none() {
            return false;
        }
        let mut sql = b"PRAGMA table_info(".to_vec();
        sql.extend_from_slice(&quote_identifier(table));
        sql.push(b')');
        let rows = self.query_rows(&sql).unwrap_or_default();
        let column = column.to_ascii_lowercase();
        let mut exists = rows.iter().any(|row| matches!(&row[1], SqlValue::Text(name) if name.to_ascii_lowercase() == column));
        if !exists && !rows.is_empty() && matches!(column.as_slice(), b"rowid" | b"oid" | b"_rowid_") {
            exists = true;
        }
        self.errcode = if exists { 0 } else { engine::SQLITE_ERROR };
        exists
    }

    pub fn is_sql_valid(&mut self, sql: &[u8]) -> bool {
        if self.connection.is_none() {
            return false;
        }
        let text = String::from_utf8_lossy(sql).into_owned();
        self.prepare_one(&text).is_ok()
    }

    pub fn database_path(directory: &Path) -> PathBuf {
        directory.join("db")
    }
}
