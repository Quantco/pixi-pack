use uv_dispatch::BuildDispatchError;
use uv_git::GitResolver;
use uv_types::{BuildArena, BuildContext, BuildIsolation};
use uv_workspace::WorkspaceCache;

/// Create a dummy build context, because we don't need to build any package.
pub struct PixiPackBuildContext {
    pub cache: uv_cache::Cache,
}

impl PixiPackBuildContext {
    pub fn new(cache: uv_cache::Cache) -> Self {
        Self { cache }
    }
}

#[allow(refining_impl_trait, unused_variables)]
impl BuildContext for PixiPackBuildContext {
    type SourceDistBuilder = uv_build_frontend::SourceBuild;

    async fn interpreter(&self) -> &uv_python::Interpreter {
        unimplemented!()
    }

    fn cache(&self) -> &uv_cache::Cache {
        &self.cache
    }

    fn git(&self) -> &GitResolver {
        unimplemented!()
    }

    fn build_arena(&self) -> &BuildArena<Self::SourceDistBuilder> {
        unimplemented!()
    }

    fn capabilities(&self) -> &uv_distribution_types::IndexCapabilities {
        unimplemented!()
    }

    fn dependency_metadata(&self) -> &uv_distribution_types::DependencyMetadata {
        unimplemented!()
    }

    fn build_options(&self) -> &uv_configuration::BuildOptions {
        unimplemented!()
    }

    fn build_isolation(&self) -> BuildIsolation<'_> {
        unimplemented!()
    }

    fn config_settings(&self) -> &uv_distribution_types::ConfigSettings {
        unimplemented!()
    }

    fn config_settings_package(&self) -> &uv_distribution_types::PackageConfigSettings {
        unimplemented!()
    }

    fn sources(&self) -> uv_configuration::SourceStrategy {
        unimplemented!()
    }

    fn locations(&self) -> &uv_distribution_types::IndexLocations {
        unimplemented!()
    }

    async fn resolve<'a>(
        &'a self,
        requirements: &'a [uv_distribution_types::Requirement],
        build_stack: &'a uv_types::BuildStack,
    ) -> anyhow::Result<uv_distribution_types::Resolution, BuildDispatchError> {
        unimplemented!()
    }

    async fn install<'a>(
        &'a self,
        resolution: &'a uv_distribution_types::Resolution,
        venv: &'a uv_python::PythonEnvironment,
        build_stack: &'a uv_types::BuildStack,
    ) -> anyhow::Result<Vec<uv_distribution_types::CachedDist>, BuildDispatchError> {
        unimplemented!()
    }

    async fn setup_build<'a>(
        &'a self,
        source: &'a std::path::Path,
        subdirectory: Option<&'a std::path::Path>,
        install_path: &'a std::path::Path,
        version_id: Option<&'a str>,
        dist: Option<&'a uv_distribution_types::SourceDist>,
        sources: uv_configuration::SourceStrategy,
        build_kind: uv_configuration::BuildKind,
        build_output: uv_configuration::BuildOutput,
        build_stack: uv_types::BuildStack,
    ) -> anyhow::Result<Self::SourceDistBuilder, BuildDispatchError> {
        unimplemented!()
    }

    async fn direct_build<'a>(
        &'a self,
        source: &'a std::path::Path,
        subdirectory: Option<&'a std::path::Path>,
        output_dir: &'a std::path::Path,
        sources: uv_configuration::SourceStrategy,
        build_kind: uv_configuration::BuildKind,
        version_id: Option<&'a str>,
    ) -> anyhow::Result<Option<uv_distribution_filename::DistFilename>, BuildDispatchError> {
        unimplemented!()
    }

    fn workspace_cache(&self) -> &WorkspaceCache {
        unimplemented!()
    }

    fn extra_build_requires(&self) -> &uv_distribution_types::ExtraBuildRequires {
        unimplemented!()
    }

    fn extra_build_variables(&self) -> &uv_distribution_types::ExtraBuildVariables {
        unimplemented!()
    }
}
