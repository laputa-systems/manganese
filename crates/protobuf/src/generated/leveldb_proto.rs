#[derive(Clone, Copy, PartialEq, Eq, Hash, ::prost::Message)]
pub struct SharedDbMetadataProto {
    #[prost(uint64, optional, tag="1")]
    pub corruptions: ::core::option::Option<u64>,
    #[prost(enumeration="shared_db_metadata_proto::MigrationStatus", optional, tag="3")]
    pub migration_status: ::core::option::Option<i32>,
    #[prost(uint32, optional, tag="4")]
    pub failure_count: ::core::option::Option<u32>,
}
pub mod shared_db_metadata_proto {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum MigrationStatus {
        MigrationNotAttempted = 0,
        MigrateToSharedSuccessful = 1,
        MigrateToSharedUniqueToBeDeleted = 2,
        MigrateToUniqueSuccessful = 3,
        MigrateToUniqueSharedToBeDeleted = 4,
    }
    impl MigrationStatus {
        pub fn as_str_name(&self) -> &'static str {
            match self {
                Self::MigrationNotAttempted => "MIGRATION_NOT_ATTEMPTED",
                Self::MigrateToSharedSuccessful => "MIGRATE_TO_SHARED_SUCCESSFUL",
                Self::MigrateToSharedUniqueToBeDeleted => "MIGRATE_TO_SHARED_UNIQUE_TO_BE_DELETED",
                Self::MigrateToUniqueSuccessful => "MIGRATE_TO_UNIQUE_SUCCESSFUL",
                Self::MigrateToUniqueSharedToBeDeleted => "MIGRATE_TO_UNIQUE_SHARED_TO_BE_DELETED",
            }
        }
        pub fn from_str_name(value: &str) -> ::core::option::Option<Self> {
            match value {
                "MIGRATION_NOT_ATTEMPTED" => Some(Self::MigrationNotAttempted),
                "MIGRATE_TO_SHARED_SUCCESSFUL" => Some(Self::MigrateToSharedSuccessful),
                "MIGRATE_TO_SHARED_UNIQUE_TO_BE_DELETED" => Some(Self::MigrateToSharedUniqueToBeDeleted),
                "MIGRATE_TO_UNIQUE_SUCCESSFUL" => Some(Self::MigrateToUniqueSuccessful),
                "MIGRATE_TO_UNIQUE_SHARED_TO_BE_DELETED" => Some(Self::MigrateToUniqueSharedToBeDeleted),
                _ => None,
            }
        }
    }
}
