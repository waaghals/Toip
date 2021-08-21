use anyhow::Result;

use crate::config::ImageSource;
use crate::oci::image::Image;

use self::build::BuildManager;
use self::path::PathManager;
use self::registry::RegistryManager;

pub mod build;
pub mod path;
pub mod registry;

pub struct ImageManager {
    registry: Option<RegistryManager>,
    path: Option<PathManager>,
    build: Option<BuildManager>,
}

impl Default for ImageManager {
    fn default() -> Self {
        ImageManager {
            registry: None,
            path: None,
            build: None,
        }
    }
}

impl ImageManager {
    pub async fn prepare(&mut self, source: &ImageSource) -> Result<Image> {
        log::info!("preparing image `{}`", source);
        match source {
            ImageSource::Registry(source) => {
                if self.registry.is_none() {
                    self.registry = Some(RegistryManager::default());
                }

                let registry = self.registry.as_ref().unwrap();
                let image = registry.pull(source).await?;
                Ok(image)
            }
            ImageSource::Path(source) => {
                if self.path.is_none() {
                    self.path = Some(PathManager::default());
                }

                let path = self.path.as_ref().unwrap();
                let image = path.convert(source).await?;
                Ok(image)
            }
            ImageSource::Build(source) => {
                if self.build.is_none() {
                    self.build = Some(BuildManager::default());
                }

                let build = self.build.as_ref().unwrap();
                let image = build.build(source).await?;
                Ok(image)
            }
        }
    }
}
