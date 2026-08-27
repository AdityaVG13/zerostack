use super::*;

impl TokenZeroEngine {
    pub fn find(
        &self,
        query: &str,
        roots: &[PathBuf],
        mode: Mode,
        max_files: usize,
        max_visible_tokens: usize,
    ) -> ToolResponse {
        self.find_with_options(
            query,
            roots,
            mode,
            max_files,
            max_visible_tokens,
            ServeOptions::default(),
        )
    }

    pub fn find_with_options(
        &self,
        query: &str,
        roots: &[PathBuf],
        mode: Mode,
        max_files: usize,
        max_visible_tokens: usize,
        options: ServeOptions,
    ) -> ToolResponse {
        crate::perf_profile::_profile_find_search(|| {
            self.search(
                "find",
                query,
                roots,
                mode,
                max_files,
                max_visible_tokens,
                options,
            )
        })
    }

    pub fn grep(
        &self,
        query: &str,
        roots: &[PathBuf],
        mode: Mode,
        max_files: usize,
        max_visible_tokens: usize,
    ) -> ToolResponse {
        self.grep_with_options(
            query,
            roots,
            mode,
            max_files,
            max_visible_tokens,
            ServeOptions::default(),
        )
    }

    pub fn grep_with_options(
        &self,
        query: &str,
        roots: &[PathBuf],
        mode: Mode,
        max_files: usize,
        max_visible_tokens: usize,
        options: ServeOptions,
    ) -> ToolResponse {
        self.search(
            "grep",
            query,
            roots,
            mode,
            max_files,
            max_visible_tokens,
            options,
        )
    }
}
