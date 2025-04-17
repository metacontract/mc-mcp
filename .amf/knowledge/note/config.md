# ReferenceConfig settings (mcp_config.toml)

- [[sources]]: List of document sources to index
    - path: Path to a directory containing Markdown files (required; relative to repo or absolute)
    - source: Source label (required; alphanumeric/hyphen, should be unique)

Constraints:
- At least one source is required
- path must be an existing directory (for now)
- source must not be empty; duplication is discouraged

Planned/future support:
1. Relative path to files in the repository (current)
2. Markdown files published on GitHub (planned: milestone 1)
3. Arbitrary URL content (planned: milestone 2; may require intermediate Markdown conversion)
