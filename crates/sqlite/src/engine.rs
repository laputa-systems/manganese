use std::cell::Cell;
use std::num::NonZero;
use std::sync::Arc;

use turso_core::Connection;
use turso_core::ConnectOpts;
use turso_core::Database;
use turso_core::DatabaseOpts;
use turso_core::Dialect;
use turso_core::LimboError;
use turso_core::OpenFlags;
use turso_core::OpenOptions;
use turso_core::PlatformIO;
use turso_core::SessionRole;
use turso_core::StepResult;
use turso_core::IO;
use turso_sqlite::SqliteDialect;

use crate::chromium_config;
use crate::sql_value::SqlValue;

pub const SQLITE_ERROR: i32 = 1;
pub const SQLITE_BUSY: i32 = 5;
pub const SQLITE_LOCKED: i32 = 6;
pub const SQLITE_MISUSE: i32 = 21;
pub const SQLITE_ROW: i32 = 100;
pub const SQLITE_DONE: i32 = 101;

pub struct Engine {
    io: Arc<dyn IO>,
    _database: Arc<Database>,
    connection: Arc<Connection>,
    views_enabled: bool,
    mmap_size: Cell<i64>,
}

pub struct EngineStatement {
    statement: turso_core::Statement,
    pub row: Vec<SqlValue>,
}

pub fn error_code(error: &LimboError) -> i32 {
    error.sqlite_extended_result_code()
}

impl Engine {
    pub fn open(path: &str, read_only: bool, triggers_enabled: bool, views_enabled: bool) -> Result<Self, i32> {
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().map_err(|error| error_code(&error))?);
        let flags = if read_only { OpenFlags::ReadOnly } else { OpenFlags::Create };
        let dialect: Arc<dyn Dialect> = Arc::new(SqliteDialect);
        let options = OpenOptions::new(dialect.with_icu_functions())
            .flags(flags)
            .db_opts(DatabaseOpts::new().with_views(true).with_vacuum(true).with_without_rowid(true).with_attach(true).with_rollback_journal(true));
        let database = Database::open(io.clone(), path, options).map_err(|error| error_code(&error))?;
        let connection = database
            .connect(ConnectOpts::new(SessionRole::Superuser))
            .map_err(|error| error_code(&error))?;
        connection.set_dqs_dml(false);
        connection.set_triggers_enabled(triggers_enabled);
        Ok(Self { io, _database: database, connection, views_enabled, mmap_size: Cell::new(0) })
    }

    fn view_names(&self) -> Vec<Vec<u8>> {
        let Ok(statement) = self.connection.prepare("SELECT name FROM sqlite_schema WHERE type = 'view'") else {
            return Vec::new();
        };
        let mut names = Vec::new();
        let mut prepared = EngineStatement { statement, row: Vec::new() };
        while let Ok(true) = self.step(&mut prepared) {
            if let Some(SqlValue::Text(name)) = prepared.row.first() {
                names.push(name.clone());
            }
        }
        names
    }

    pub fn prepare(&self, sql: &str) -> Result<(EngineStatement, usize), i32> {
        let first = chromium_config::first_statement(sql);
        if chromium_config::omits(&first.words) {
            return Err(SQLITE_ERROR);
        }
        if !self.views_enabled && chromium_config::names_view(&first.words, &self.view_names()) {
            return Err(SQLITE_ERROR);
        }
        if chromium_config::is_omitted_pragma(&first.words) {
            let statement = self.connection.prepare("SELECT 1 WHERE 0").map_err(|error| error_code(&error))?;
            return Ok((EngineStatement { statement, row: Vec::new() }, first.end));
        }
        if let Some(pragma) = chromium_config::config_pragma(&first.words) {
            let value = match pragma {
                chromium_config::ConfigPragma::Constant(value) => value,
                chromium_config::ConfigPragma::MmapSize(Some(size)) => {
                    self.mmap_size.set(size);
                    size
                }
                chromium_config::ConfigPragma::MmapSize(None) => self.mmap_size.get(),
            };
            let statement = self.connection.prepare(format!("SELECT {value}")).map_err(|error| error_code(&error))?;
            return Ok((EngineStatement { statement, row: Vec::new() }, first.end));
        }
        let statement = self.connection.prepare(sql).map_err(|error| error_code(&error))?;
        let tail = statement.tail_offset();
        Ok((EngineStatement { statement, row: Vec::new() }, tail))
    }

    pub fn changes(&self) -> i64 {
        self.connection.changes()
    }

    pub fn last_insert_rowid(&self) -> i64 {
        self.connection.last_insert_rowid()
    }

    pub fn autocommit(&self) -> bool {
        self.connection.get_auto_commit()
    }

    pub fn step(&self, statement: &mut EngineStatement) -> Result<bool, i32> {
        loop {
            match statement.statement.step_with_structured_errors() {
                Ok(StepResult::Row) => {
                    let row = statement.statement.row().expect("a stepped row");
                    statement.row = row.get_values().map(|value| SqlValue::from_turso(value.as_ref())).collect();
                    return Ok(true);
                }
                Ok(StepResult::Done) => {
                    statement.row.clear();
                    return Ok(false);
                }
                Ok(StepResult::IO | StepResult::Yield | StepResult::Sleep { .. }) => {
                    self.io.step().map_err(|error| error_code(&error))?;
                }
                Ok(StepResult::Busy | StepResult::Interrupt) => {
                    statement.row.clear();
                    return Err(5);
                }
                Err(error) => {
                    statement.row.clear();
                    return Err(error_code(&error));
                }
            }
        }
    }

    pub fn reset(&self, statement: &mut EngineStatement) {
        statement.row.clear();
        statement.statement.reset_best_effort();
    }

    pub fn clear_bindings(&self, statement: &mut EngineStatement) {
        statement.statement.clear_bindings();
    }

    pub fn bind(&self, statement: &mut EngineStatement, index: usize, value: &SqlValue) {
        if let Some(index) = NonZero::new(index) {
            if index.get() <= statement.statement.parameters_count() {
                let _ = statement.statement.bind_at(index, value.to_turso());
            }
        }
    }

    pub fn close(self) {
        let _ = self.connection.close();
    }
}
