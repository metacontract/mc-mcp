use serde::Deserialize;
use figment::{Figment, providers::{Format, Toml, Env, Serialized}};
use std::path::PathBuf;

#[derive(Deserialize, Debug, Clone)]
pub enum SourceType {
    #[serde(rename = "local")]
    Local,
    // TODO: Add Git, Http later
}

#[derive(Deserialize, Debug, Clone)]
pub struct DocumentSource {
    pub name: String, // Identifier for the source (e.g., "mc-docs", "my-project-docs")
    pub source_type: SourceType,
    pub path: PathBuf, // For Local type
    // pub url: Option<String>, // For Git/Http types
    // pub branch: Option<String>, // For Git type
}

#[derive(Deserialize, Debug, Clone)]
pub struct ReferenceConfig {
    // Default source is always mc-docs, maybe handled separately or require explicit definition?
    // Let's require explicit definition for now for clarity.
    pub sources: Vec<DocumentSource>,
    #[serde(default = "default_prebuilt_index_path")]
    pub prebuilt_index_path: Option<PathBuf>, // Path relative to mc-docs source path? Or absolute? Let's try relative to config for now.
    // TODO: Add embedding model config, Qdrant config here? Or keep separate?
    // Keep separate for now, loaded via env vars in main.rs
}

fn default_prebuilt_index_path() -> Option<PathBuf> {
    None
}

#[derive(Deserialize, Debug, Clone)]
pub struct McpConfig {
    #[serde(default)]
    pub reference: ReferenceConfig,
    // Add other config sections like ToolConfig later
}

// Provide a default implementation for ReferenceConfig
impl Default for ReferenceConfig {
    fn default() -> Self {
        ReferenceConfig {
            sources: vec![], // Start with no explicitly configured sources by default
            prebuilt_index_path: None,
        }
    }
}


// Example function to load config (will be used in main.rs)
pub fn load_config() -> Result<McpConfig, figment::Error> {
    Figment::new()
        // Default values for McpConfig structure fields
        .merge(Serialized::defaults(McpConfig {
            reference: ReferenceConfig::default(),
        }))
        // Load from `mcp_config.toml` in the current directory
        .merge(Toml::file("mcp_config.toml").nested())
        // Load `MCP_` prefixed environment variables
        .merge(Env::prefixed("MCP_").ignore_underscores(true))
        .extract()
}

#[cfg(test)]
mod tests {
    use super::*;
    use figment::Jail;

    #[test]
    fn test_load_config_defaults() {
        Jail::expect_with(|jail| {
            // No config file
            let config = load_config().expect("Failed to load default config");
            assert!(config.reference.sources.is_empty());
            assert!(config.reference.prebuilt_index_path.is_none());
            Ok(())
        });
    }

    #[test]
    fn test_load_config_from_file() {
        Jail::expect_with(|jail| {
            jail.create_file(
                "mcp_config.toml",
                r#"
                [reference]
                prebuilt_index_path = "my_index.json"
                sources = [
                    { name = "mc", source_type = "local", path = "metacontract/mc/site/docs" },
                    { name = "extra", source_type = "local", path = "./local_docs" },
                ]
                "#,
            )?;

            let config = load_config().expect("Failed to load config from file");

            assert_eq!(config.reference.sources.len(), 2);
            assert_eq!(config.reference.sources[0].name, "mc");
            assert_eq!(config.reference.sources[0].path, PathBuf::from("metacontract/mc/site/docs"));
            assert!(matches!(config.reference.sources[0].source_type, SourceType::Local));

            assert_eq!(config.reference.sources[1].name, "extra");
            assert_eq!(config.reference.sources[1].path, PathBuf::from("./local_docs"));
            assert!(matches!(config.reference.sources[1].source_type, SourceType::Local));

            assert_eq!(config.reference.prebuilt_index_path, Some(PathBuf::from("my_index.json")));

            Ok(())
        });
    }

     #[test]
    fn test_load_config_env_override() {
        Jail::expect_with(|jail| {
            jail.create_file(
                "mcp_config.toml",
                r#"
                [reference]
                sources = [
                    { name = "mc", source_type = "local", path = "original/path" },
                ]
                "#,
            )?;
            // Override sources via JSON string in env var
             jail.set_env("MCP_REFERENCE_SOURCES", "[{ name = \"env_mc\", source_type = \"local\", path = \"env/path\" }]");
             jail.set_env("MCP_REFERENCE_PREBUILT_INDEX_PATH", "env_index.json");


            let config = load_config().expect("Failed to load config with env override");

            assert_eq!(config.reference.sources.len(), 1);
            assert_eq!(config.reference.sources[0].name, "env_mc");
            assert_eq!(config.reference.sources[0].path, PathBuf::from("env/path"));
             assert_eq!(config.reference.prebuilt_index_path, Some(PathBuf::from("env_index.json")));

            Ok(())
        });
    }

     #[test]
     fn test_load_config_partial_env_override() {
         Jail::expect_with(|jail| {
             jail.create_file(
                 "mcp_config.toml",
                 r#"
                 [reference]
                 prebuilt_index_path = "file_index.json" # This should be kept
                 sources = [
                     { name = "mc", source_type = "local", path = "original/path" },
                 ]
                 "#,
             )?;
             // Only override sources
              jail.set_env("MCP_REFERENCE_SOURCES", "[{ name = \"env_mc\", source_type = \"local\", path = \"env/path\" }]");


             let config = load_config().expect("Failed to load config with partial env override");

             assert_eq!(config.reference.sources.len(), 1);
             assert_eq!(config.reference.sources[0].name, "env_mc");
             assert_eq!(config.reference.prebuilt_index_path, Some(PathBuf::from("file_index.json"))); // Check it kept the file value

             Ok(())
         });
     }
}
