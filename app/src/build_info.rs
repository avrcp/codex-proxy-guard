//! Embedded build provenance. Values come from the packaging script through
//! environment variables at compile time; absent values are honestly
//! reported as unknown/dirty, never fabricated.

pub fn product_version() -> &'static str {
    option_env!("CPG_PRODUCT_VERSION").unwrap_or("unknown")
}

pub fn build_commit() -> &'static str {
    option_env!("CPG_BUILD_COMMIT").unwrap_or("unknown")
}

pub fn build_dirty() -> bool {
    !matches!(option_env!("CPG_BUILD_DIRTY"), Some("false"))
}
