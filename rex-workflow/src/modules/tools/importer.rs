use crate::{
    modules::tools::{ffmpeg, imagemagick, poppler, qpdf},
    state::State,
};
use futures::future::BoxFuture;
use rex::engine::{
    EngineError, ImportRequest, Importer, Module, ResolvedModule, ResolvedModuleContent,
};
use std::sync::Arc;

type ModuleFactory = fn() -> Result<Module<State>, EngineError>;

#[derive(Clone, Copy)]
struct ToolRegistration {
    module_id: &'static str,
    factory: ModuleFactory,
}

impl ToolRegistration {
    const fn new(module_id: &'static str, factory: ModuleFactory) -> Self {
        Self { module_id, factory }
    }
}

// Keep registrations sorted by module ID so ToolImporter can use binary search.
static TOOL_REGISTRY: &[ToolRegistration] = &[
    ToolRegistration::new("tools.ffmpeg", ffmpeg::module),
    ToolRegistration::new("tools.imagemagick", imagemagick::module),
    ToolRegistration::new("tools.poppler", poppler::module),
    ToolRegistration::new("tools.qpdf", qpdf::module),
];

struct ToolImporter {
    registry: &'static [ToolRegistration],
}

impl ToolImporter {
    const fn new(registry: &'static [ToolRegistration]) -> Self {
        Self { registry }
    }
}

impl Importer<State> for ToolImporter {
    fn import<'a>(
        &'a self,
        request: ImportRequest,
    ) -> BoxFuture<'a, Result<Option<ResolvedModule<State>>, EngineError>> {
        Box::pin(async move {
            let requested_id = request.module_id.to_string();
            let Ok(index) = self
                .registry
                .binary_search_by(|entry| entry.module_id.cmp(requested_id.as_str()))
            else {
                return Ok(None);
            };

            let module = (self.registry[index].factory)()?;
            Ok(Some(ResolvedModule {
                id: request.module_id,
                content: ResolvedModuleContent::module(module),
            }))
        })
    }
}

pub(crate) fn importer() -> Arc<dyn Importer<State>> {
    Arc::new(ToolImporter::new(TOOL_REGISTRY))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::store::Store;
    use rex::{
        engine::{Builder, CompileOptions, ModuleId},
        parser::parse as parse_rex,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    static USED_CALLS: AtomicUsize = AtomicUsize::new(0);
    static UNUSED_CALLS: AtomicUsize = AtomicUsize::new(0);

    fn assert_documented(module: &Module<State>) {
        assert!(module.docs().is_some_and(|docs| !docs.trim().is_empty()));
        for export in module.exports() {
            assert!(
                export.docs().is_some_and(|docs| !docs.trim().is_empty()),
                "export `{}` is missing documentation",
                export.name
            );
            assert!(
                export.params().all(|param| !param.starts_with("arg")),
                "export `{}` has generated parameter names",
                export.name
            );
        }
        for staged in module.adts() {
            assert!(
                staged
                    .adt
                    .docs
                    .as_deref()
                    .is_some_and(|docs| !docs.trim().is_empty()),
                "ADT `{}` is missing documentation",
                staged.adt.name
            );
        }
    }

    fn used_module() -> Result<Module<State>, EngineError> {
        USED_CALLS.fetch_add(1, Ordering::SeqCst);
        let mut module = Module::new("tools.test_used", None);
        module.export("inc", |_state: State, value: i32| {
            Ok::<i32, EngineError>(value + 1)
        })?;
        Ok(module)
    }

    fn unused_module() -> Result<Module<State>, EngineError> {
        UNUSED_CALLS.fetch_add(1, Ordering::SeqCst);
        Ok(Module::new("tools.test_unused", None))
    }

    static TEST_REGISTRY: &[ToolRegistration] = &[
        ToolRegistration::new("tools.test_unused", unused_module),
        ToolRegistration::new("tools.test_used", used_module),
    ];

    #[test]
    fn production_registry_is_sorted_and_unique() {
        assert!(
            TOOL_REGISTRY
                .windows(2)
                .all(|pair| pair[0].module_id < pair[1].module_id)
        );
    }

    #[test]
    fn production_tool_rex_apis_are_fully_documented() {
        let ffmpeg = ffmpeg::module().unwrap();
        assert_documented(&ffmpeg);
        let ffmpeg_docs = ffmpeg.docs().unwrap();
        assert!(ffmpeg_docs.contains("Headless FFmpeg and FFprobe tools"));
        assert!(ffmpeg_docs.contains("content hashes rather than host paths"));

        let imagemagick = imagemagick::module().unwrap();
        assert_documented(&imagemagick);
        let imagemagick_docs = imagemagick.docs().unwrap();
        assert!(imagemagick_docs.contains("Semantic ImageMagick tools"));
        assert!(imagemagick_docs.contains("exact ImageMagick command-line order"));

        let poppler = poppler::module().unwrap();
        assert_documented(&poppler);
        let poppler_docs = poppler.docs().unwrap();
        assert!(poppler_docs.contains("Poppler command-line utilities"));
        assert!(poppler_docs.contains("content-addressed"));

        let qpdf = qpdf::module().unwrap();
        assert_documented(&qpdf);
        let qpdf_docs = qpdf.docs().unwrap();
        assert!(qpdf_docs.contains("QPDF structural transformations"));
        assert!(qpdf_docs.contains("content-addressed"));
    }

    #[test]
    fn production_tool_rex_apis_are_headless() {
        let ffmpeg = ffmpeg::module().unwrap();
        assert!(ffmpeg.exports().iter().all(|export| export.name != "play"));
        assert!(
            ffmpeg.adts().iter().all(|staged| {
                !matches!(staged.adt.name.as_str(), "DeviceSource" | "PlayOption")
            })
        );
        assert!(ffmpeg.adts().iter().all(|staged| {
            staged
                .adt
                .variants
                .iter()
                .all(|variant| variant.name.as_str() != "CaptureDevice")
        }));

        let imagemagick = imagemagick::module().unwrap();
        assert!(
            imagemagick.exports().iter().all(|export| {
                !matches!(export.name.as_str(), "capture" | "display" | "animate")
            })
        );
        assert!(imagemagick.adts().iter().all(|staged| {
            !matches!(
                staged.adt.name.as_str(),
                "CaptureTarget" | "CaptureOption" | "DisplayOption" | "AnimateOption"
            ) && staged.adt.variants.iter().all(|variant| {
                !matches!(
                    variant.name.as_str(),
                    "OtherBuiltinImage" | "SettingFontFamily" | "DrawFontFamily"
                )
            })
        }));
    }

    #[tokio::test]
    async fn unknown_module_is_not_claimed() {
        let importer = ToolImporter::new(TOOL_REGISTRY);
        let request = ImportRequest::new(ModuleId::parse("tools.unknown").unwrap());

        assert!(importer.import(request).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn only_requested_module_is_built_once_per_compilation() {
        USED_CALLS.store(0, Ordering::SeqCst);
        UNUSED_CALLS.store(0, Ordering::SeqCst);

        let state = State::local(Store::new_in_memory());
        let mut builder = Builder::with_prelude(state).unwrap();
        builder.add_importer(Arc::new(ToolImporter::new(TEST_REGISTRY)));
        let compiler = builder.build_compiler();
        let program = parse_rex(
            r#"
                import tools.test_used as A;
                import tools.test_used as B;
                A.inc 1 + B.inc 2
            "#,
        )
        .unwrap();
        let options = CompileOptions::new(ModuleId::parse("main").unwrap());

        compiler.compile_program(&program, options).await.unwrap();

        assert_eq!(USED_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(UNUSED_CALLS.load(Ordering::SeqCst), 0);
    }
}
