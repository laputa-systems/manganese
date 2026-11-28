use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

use sha2::Digest;
use sha2::Sha256;

use crate::database::Database;
use crate::engine;
use crate::database::DatabaseOptions;
use crate::database::StatementId;
use crate::meta_table::MetaTable;
use crate::meta_table::MetaValue;
use crate::sql_value::ColumnType;
use crate::sql_value::SqlValue;
use crate::wire;
use crate::wire::Reader;
use crate::wire::Writer;

const SUFFIXES: [&str; 3] = ["", "-journal", "-wal"];

#[derive(Clone, Copy)]
struct TransactionSlot {
    slot: u8,
    epoch: u64,
    active: bool,
}

pub struct Session {
    root: PathBuf,
    generation: u32,
    database: Option<Database>,
    abandoned: Vec<Database>,
    statements: BTreeMap<u8, StatementId>,
    transactions: BTreeMap<u8, TransactionSlot>,
    transaction_stack: Vec<u8>,
    meta: bool,
}

fn read_whole_file(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

impl Session {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            generation: 0,
            database: None,
            abandoned: Vec::new(),
            statements: BTreeMap::new(),
            transactions: BTreeMap::new(),
            transaction_stack: Vec::new(),
            meta: false,
        }
    }

    pub fn run(mut self, request: &[u8]) -> Vec<u8> {
        let mut input = Reader::new(request);
        let mut out = Writer::default();
        if std::fs::create_dir_all(self.directory()).is_err() {
            wire::fail("cannot create request directory");
        }
        let steps = input.u16();
        for _ in 0..steps {
            self.step(&mut input, &mut out);
        }
        if !input.done() {
            wire::fail("trailing request bytes");
        }
        self.teardown();
        out.into_inner()
    }

    fn teardown(&mut self) {
        self.clear_transactions();
        self.clear_statements();
        self.meta = false;
        if let Some(mut database) = self.database.take() {
            database.close();
            if database.has_connection() {
                database.close();
            }
        }
        for mut database in self.abandoned.drain(..) {
            database.close();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }

    fn directory(&self) -> PathBuf {
        self.root.join(format!("g{}", self.generation))
    }

    fn database_path(&self) -> PathBuf {
        Database::database_path(&self.directory())
    }

    fn sidecar_path(&self, kind: usize) -> PathBuf {
        let mut path = self.database_path().into_os_string();
        path.push(SUFFIXES[kind]);
        PathBuf::from(path)
    }

    fn db(&mut self) -> &mut Database {
        match self.database.as_mut() {
            Some(database) => database,
            None => wire::fail("no database object"),
        }
    }

    fn slot(&self, slot: u8) -> StatementId {
        match self.statements.get(&slot) {
            Some(id) => *id,
            None => wire::fail("unknown statement slot"),
        }
    }

    fn result(&mut self, out: &mut Writer, ok: bool) {
        out.bool(ok);
        let code = if ok { 0 } else { self.db().error_code() };
        if !ok {
            self.db().clear_step_error();
            if code > 25 {
                self.db().consume_statement_errors(code);
            }
        }
        out.i32(code);
    }

    fn result_step(&mut self, out: &mut Writer, ok: bool) {
        out.bool(ok);
        out.i32(if ok { 0 } else { self.db().error_code() });
    }

    fn drop_statement(&mut self, id: StatementId) {
        if let Some(database) = self.database.as_mut() {
            database.finalize(id);
        }
    }

    fn drop_transaction(&mut self, slot: TransactionSlot) {
        if !slot.active {
            return;
        }
        self.transaction_stack.retain(|value| *value != slot.slot);
        if let Some(database) = self.database.as_mut() {
            if database.epoch() == slot.epoch && database.is_open() {
                database.rollback_slot(slot.slot);
            }
        }
    }

    fn clear_transactions(&mut self) {
        let slots: Vec<TransactionSlot> = std::mem::take(&mut self.transactions).into_values().collect();
        for slot in slots {
            self.drop_transaction(slot);
        }
        self.transaction_stack.clear();
    }

    fn clear_statements(&mut self) {
        let ids: Vec<StatementId> = std::mem::take(&mut self.statements).into_values().collect();
        for id in ids {
            self.drop_statement(id);
        }
    }

    fn write_value(out: &mut Writer, value: &SqlValue, getter: u8) {
        match getter {
            1 => {
                out.u8(1);
                out.i64(value.int64());
            }
            2 => {
                out.u8(2);
                out.u64(value.double().to_bits());
            }
            3 => {
                out.u8(3);
                out.bytes(&value.text());
            }
            4 => {
                out.u8(4);
                out.bytes(&value.text());
            }
            5 => {
                out.u8(1);
                out.i64(i64::from(value.int64() as i32));
            }
            6 => {
                out.u8(1);
                out.i64(i64::from(value.int64() != 0));
            }
            _ => match value.column_type() {
                ColumnType::Integer => Self::write_value(out, value, 1),
                ColumnType::Float => Self::write_value(out, value, 2),
                ColumnType::Text => Self::write_value(out, value, 3),
                ColumnType::Blob => Self::write_value(out, value, 4),
                ColumnType::Null => out.u8(0),
            },
        }
    }

    fn write_row(rows: &mut Writer, row: &[SqlValue], getters: &[u8]) {
        rows.u16(row.len() as u16);
        for (column, value) in row.iter().enumerate() {
            Self::write_value(rows, value, getters.get(column).copied().unwrap_or(0));
        }
    }

    fn step(&mut self, input: &mut Reader<'_>, out: &mut Writer) {
        let op = input.u8();
        out.u8(op);
        if self.database.is_some() {
            let slot = if self.transaction_stack.len() == 1 {
                self.transaction_stack.first().copied()
            } else {
                None
            };
            self.db().set_transaction_slot(slot);
        }
        match op {
            0x01 => {
                let kind = usize::from(input.u8());
                if kind > 2 {
                    wire::fail("unknown file kind");
                }
                let contents = input.bytes();
                if std::fs::write(self.sidecar_path(kind), contents).is_err() {
                    wire::fail("cannot write database file");
                }
                out.bool(true);
            }
            0x02 => {
                let flags = input.u8();
                let page_size = input.u32();
                let cache_size = input.i32();
                self.clear_transactions();
                self.clear_statements();
                self.meta = false;
                if let Some(mut previous) = self.database.take() {
                    previous.close();
                }
                let mut database = Database::new(DatabaseOptions::from_flags(flags, page_size, cache_size));
                let ok = database.open(&self.database_path());
                self.database = Some(database);
                self.result(out, ok);
            }
            0x03 => {
                self.clear_transactions();
                self.clear_statements();
                self.meta = false;
                self.db().close();
                out.bool(true);
            }
            0x04 => {
                let sql = input.bytes();
                let ok = self.db().execute(&String::from_utf8_lossy(&sql));
                self.result(out, ok);
            }
            0x05 => {
                let slot = input.u8();
                let sql = input.bytes();
                if let Some(previous) = self.statements.remove(&slot) {
                    let old_error = self.db().last_result(previous);
                    let id = self.db().unique_statement(&sql);
                    self.drop_statement(previous);
                    self.statements.insert(slot, id);
                    if let Some(code) = old_error {
                        if code != engine::SQLITE_ROW && code != engine::SQLITE_DONE {
                            self.db().set_error(code);
                        }
                    }
                } else {
                    let id = self.db().unique_statement(&sql);
                    self.statements.insert(slot, id);
                }
                let id = self.slot(slot);
                let valid = self.db().is_valid(id);
                self.result(out, valid);
            }
            0x06 => {
                let id = self.slot(input.u8());
                let index = usize::from(input.u16());
                let value = match input.u8() {
                    0 => SqlValue::Null,
                    1 => SqlValue::Integer(input.u64() as i64),
                    2 => SqlValue::Float(f64::from_bits(input.u64())),
                    3 => SqlValue::Text(input.bytes()),
                    4 => SqlValue::Blob(input.bytes()),
                    _ => wire::fail("unknown value tag"),
                };
                let value = match value {
                    SqlValue::Float(value) if value.is_nan() => SqlValue::Null,
                    other => other,
                };
                self.db().bind(id, index, &value);
                out.bool(true);
            }
            0x07 => {
                let id = self.slot(input.u8());
                let max_rows = input.u16();
                let getter_count = input.u8();
                let getters: Vec<u8> = (0..getter_count).map(|_| input.u8()).collect();
                let mut rows = Writer::default();
                let mut row_count: u32 = 0;
                let mut more = false;
                while row_count < u32::from(max_rows) {
                    more = self.db().step(id);
                    if !more {
                        break;
                    }
                    row_count += 1;
                    let row = self.db().row(id).to_vec();
                    Self::write_row(&mut rows, &row, &getters);
                }
                out.u32(row_count);
                out.bytes(&rows.into_inner());
                out.bool(more);
                let succeeded = self.db().succeeded(id);
                self.result_step(out, succeeded || more);
            }
            0x08 => {
                let id = self.slot(input.u8());
                let clear = input.u8() != 0;
                self.db().reset(id, clear);
                out.bool(true);
            }
            0x09 => {
                let slot = input.u8();
                if let Some(id) = self.statements.remove(&slot) {
                    let code = self.db().final_error(id);
                    self.drop_statement(id);
                    if code > 25 {
                        self.db().set_error(code);
                    }
                }
                out.bool(true);
            }
            0x0a => {
                let slot = input.u8();
                let epoch = self.db().epoch();
                let ok = self.db().is_open() && self.db().begin_transaction();
                let sole_slot = self.transactions.len() == 1;
                if let Some(previous) = self.transactions.insert(slot, TransactionSlot { slot, epoch, active: ok }) {
                    if sole_slot && self.db().schema_write_slot() == Some(previous.slot) {
                        self.db().clear_schema_write_slot(previous.slot);
                    }
                    self.drop_transaction(previous);
                }
                if ok {
                    self.transaction_stack.push(slot);
                }
                self.result(out, ok);
            }
            0x0b => {
                let slot = input.u8();
                let Some(transaction) = self.transactions.get_mut(&slot) else {
                    wire::fail("unknown transaction slot");
                };
                transaction.active = false;
                let epoch = transaction.epoch;
                self.transaction_stack.retain(|value| *value != slot);
                let database = self.db();
                let ok = database.epoch() == epoch && database.has_connection() && database.commit_transaction();
                self.result(out, ok);
            }
            0x0c => {
                let slot = input.u8();
                let Some(transaction) = self.transactions.get_mut(&slot) else {
                    wire::fail("unknown transaction slot");
                };
                transaction.active = false;
                let epoch = transaction.epoch;
                self.transaction_stack.retain(|value| *value != slot);
                let database = self.db();
                if database.epoch() == epoch && database.has_connection() {
                    database.rollback_explicit(slot);
                }
                out.bool(true);
            }
            0x0d => {
                let slot = input.u8();
                if let Some(transaction) = self.transactions.remove(&slot) {
                    self.drop_transaction(transaction);
                }
                out.bool(true);
            }
            0x0e => {
                let version = input.i32();
                let compatible = input.i32();
                self.meta = true;
                let ok = MetaTable::init(self.db(), version, compatible);
                self.result(out, ok);
            }
            0x0f => {
                let version = input.i32();
                self.require_meta();
                let ok = MetaTable::set_version(self.db(), version);
                self.result(out, ok);
            }
            0x10 => {
                self.require_meta();
                out.bool(true);
                let version = MetaTable::version(self.db());
                out.i32(version);
            }
            0x11 => {
                let version = input.i32();
                self.require_meta();
                let ok = MetaTable::set_compatible_version(self.db(), version);
                self.result(out, ok);
            }
            0x12 => {
                self.require_meta();
                out.bool(true);
                let version = MetaTable::compatible_version(self.db());
                out.i32(version);
            }
            0x13 => {
                let key = input.bytes();
                let value = match input.u8() {
                    1 => MetaValue::Integer(input.u64() as i64),
                    3 => MetaValue::Text(input.bytes()),
                    _ => wire::fail("meta values are integers or text"),
                };
                self.require_meta();
                let ok = MetaTable::set_value(self.db(), &key, value);
                self.result(out, ok);
            }
            0x14 => {
                let key = input.bytes();
                let tag = input.u8();
                self.require_meta();
                match tag {
                    1 => {
                        let value = MetaTable::int_value(self.db(), &key);
                        out.bool(value.is_some());
                        out.i64(value.unwrap_or(0));
                    }
                    3 => {
                        let value = MetaTable::text_value(self.db(), &key);
                        out.bool(value.is_some());
                        out.bytes(&value.unwrap_or_default());
                    }
                    _ => wire::fail("meta values are integers or text"),
                }
            }
            0x15 => {
                let key = input.bytes();
                self.require_meta();
                let ok = MetaTable::delete_key(self.db(), &key);
                self.result(out, ok);
            }
            0x16 => {
                let lowest = input.i32();
                let current = input.i32();
                out.bool(true);
                let result = MetaTable::raze_if_incompatible(self.db(), lowest, current);
                out.u8(result as u8);
            }
            0x17 => {
                let ok = self.db().raze();
                self.result(out, ok);
            }
            0x18 => {
                self.clear_transactions();
                self.clear_statements();
                self.meta = false;
                let ok = self.db().raze_and_poison();
                out.bool(ok);
            }
            0x19 => {
                self.db().poison();
                out.bool(true);
            }
            0x1a => {
                let ok = self.db().vacuum();
                self.result(out, ok);
            }
            0x1b => {
                let truncate = input.u8() != 0;
                let ok = self.db().checkpoint(truncate);
                out.bool(ok);
            }
            0x1c => {
                let kind = input.u8();
                let name = input.bytes();
                let exists = match kind {
                    0 => self.db().does_table_exist(&name),
                    1 => self.db().does_index_exist(&name),
                    2 => self.db().does_view_exist(&name),
                    3 => {
                        let column = input.bytes();
                        self.db().does_column_exist(&name, &column)
                    }
                    4 => MetaTable::does_table_exist(self.db()),
                    _ => wire::fail("unknown schema item kind"),
                };
                out.bool(exists);
            }
            0x1d => {
                out.bool(true);
                let changes = self.db().last_change_count();
                let rowid = self.db().last_insert_row_id();
                out.i64(changes);
                out.i64(rowid);
            }
            0x1e => self.dump(out),
            0x1f => {
                out.bool(true);
                for kind in 0..3 {
                    let size = std::fs::metadata(self.sidecar_path(kind)).ok().map(|metadata| metadata.len());
                    out.bool(size.is_some());
                    out.i64(size.unwrap_or(0) as i64);
                }
            }
            0x20 => {
                let contents = read_whole_file(&self.database_path());
                out.bool(contents.is_some());
                out.bytes(&contents.map(|data| Sha256::digest(&data).to_vec()).unwrap_or_default());
            }
            0x21 => {
                let pattern = input.bytes();
                let mut found = false;
                for kind in 0..3 {
                    if let Some(contents) = read_whole_file(&self.sidecar_path(kind)) {
                        if pattern.is_empty() || contents.windows(pattern.len()).any(|window| window == pattern) {
                            found = true;
                        }
                    }
                }
                out.bool(true);
                out.bool(found);
            }
            0x22 => {
                let action = input.u8();
                let files: Vec<Option<Vec<u8>>> = (0..3).map(|kind| read_whole_file(&self.sidecar_path(kind))).collect();
                self.clear_transactions();
                self.clear_statements();
                self.meta = false;
                if let Some(database) = self.database.take() {
                    self.abandoned.push(database);
                }
                self.generation += 1;
                if std::fs::create_dir_all(self.directory()).is_err() {
                    wire::fail("cannot create crash directory");
                }
                for (kind, contents) in files.into_iter().enumerate() {
                    let Some(contents) = contents else { continue };
                    if kind > 0 && action == 1 {
                        continue;
                    }
                    if std::fs::write(self.sidecar_path(kind), contents).is_err() {
                        wire::fail("cannot write crash image");
                    }
                }
                out.bool(true);
            }
            0x23 => {
                out.bool(true);
                for kind in 0..3 {
                    let contents = read_whole_file(&self.sidecar_path(kind));
                    out.bool(contents.is_some());
                    out.bytes(&contents.unwrap_or_default());
                }
            }
            0x24 => {
                let sql = input.bytes();
                let valid = self.db().is_sql_valid(&sql);
                out.bool(valid);
            }
            _ => wire::fail("unknown step op"),
        }
    }

    fn require_meta(&self) {
        if !self.meta {
            wire::fail("meta table not initialized");
        }
    }

    fn dump_query(&mut self, out: &mut Writer, sql: &[u8]) {
        let database = self.db();
        let id = database.unique_statement(sql);
        let mut rows = Writer::default();
        let mut count: u32 = 0;
        while database.step(id) {
            count += 1;
            let row = database.row(id).to_vec();
            rows.u16(row.len() as u16);
            for value in &row {
                Self::write_value(&mut rows, value, 0);
            }
        }
        let succeeded = database.succeeded(id);
        database.finalize(id);
        out.bool(succeeded);
        out.u32(count);
        out.bytes(&rows.into_inner());
    }

    fn dump(&mut self, out: &mut Writer) {
        out.bool(true);
        self.dump_query(out, b"PRAGMA page_size");
        self.dump_query(out, b"PRAGMA user_version");
        self.dump_query(out, b"PRAGMA application_id");
        self.dump_query(out, b"SELECT type, name, tbl_name, sql FROM sqlite_schema ORDER BY rowid");
        let mut tables = Vec::new();
        {
            let database = self.db();
            let id = database.unique_statement(b"SELECT name, sql FROM sqlite_schema WHERE type = 'table' ORDER BY rowid");
            while database.step(id) {
                let row = database.row(id).to_vec();
                let sql = row[1].text();
                if sql.starts_with(b"CREATE VIRTUAL TABLE") {
                    continue;
                }
                let contains = |needle: &[u8]| sql.windows(needle.len()).any(|window| window == needle);
                let without_rowid = contains(b"WITHOUT ROWID") || contains(b"without rowid");
                tables.push((row[0].text(), without_rowid));
            }
            database.finalize(id);
        }
        out.u32(tables.len() as u32);
        for (name, without_rowid) in tables {
            let mut quoted = b"\"".to_vec();
            for &byte in &name {
                quoted.push(byte);
                if byte == b'"' {
                    quoted.push(b'"');
                }
            }
            quoted.push(b'"');
            out.bytes(&name);
            let mut sql = b"SELECT * FROM ".to_vec();
            sql.extend_from_slice(&quoted);
            if !without_rowid {
                sql.extend_from_slice(b" ORDER BY rowid");
            }
            self.dump_query(out, &sql);
        }
    }
}
