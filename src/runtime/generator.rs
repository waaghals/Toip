use std::collections::HashMap;
use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::prelude::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::dirs::container_dir;
use crate::image::manager::ImageManager;
use crate::oci::image::Image;
use crate::{config::ContainerConfig, dirs::layer_dir};
use anyhow::{Context, Result};
use async_trait::async_trait;
use nix::unistd::{self, Gid, Group, Uid, User};
use oci_spec::{
    Linux, LinuxIdMapping, LinuxNamespace, LinuxNamespaceType, Mount, Process, Root, Spec,
};

const BIN_DIR: &str = "/usr/bin/doe";

#[async_trait]
pub trait RuntimeBundleGenerator {
    async fn build<S, I>(
        &self,
        container_id: S,
        config: &ContainerConfig,
        arguments: I,
    ) -> Result<PathBuf>
    where
        S: Into<String> + Send,
        I: IntoIterator<Item = String> + Send;
}

pub struct RunGenerator {
    layer_dir: PathBuf,
    container_dir: PathBuf,
}

impl RunGenerator {
    fn container_dir(&self, container_name: &str) -> PathBuf {
        let mut directory = self.container_dir.clone();
        directory.push(container_name);
        directory
    }

    fn location(&self, container_name: &str, sub_directory: &str) -> PathBuf {
        let mut directory = self.container_dir(container_name);
        directory.push(sub_directory);
        directory
    }
}

impl Default for RunGenerator {
    fn default() -> Self {
        RunGenerator {
            layer_dir: layer_dir(),
            container_dir: container_dir(),
        }
    }
}

fn overlay_option<S>(kind: S, paths: &[PathBuf]) -> String
where
    S: Into<String> + Send,
{
    let mut option = kind.into();
    option.push('=');
    let str_paths: Vec<String> = paths
        .iter()
        .cloned() // TODO cleanup this mess
        .map(|path| path.into_os_string())
        .map(|os_string| os_string.into_string().unwrap())
        .collect();
    let joined = str_paths.join(":");
    option.push_str(&joined);

    option
}

fn create_directories(directories: &[PathBuf]) -> Result<()> {
    for directory in directories {
        fs::create_dir_all(directory)?;
    }
    Ok(())
}

// TODO improve error handling here
// TODO cleanup nesting
fn resolve_user(image: Image) -> (Uid, Gid) {
    let root_user = Uid::from_raw(0);
    let root_group = Gid::from_raw(0);

    match image.config {
        Some(config) => match config.user {
            Some(user) => {
                let mut parts = user.split(':');
                let user_name_or_id = parts.next().unwrap();
                let group_name_or_id = parts.next();

                let user_obj = User::from_name(user_name_or_id).unwrap();

                let mut group_obj = None;
                if let Some(group_name_or_id) = group_name_or_id {
                    group_obj = Group::from_name(group_name_or_id).unwrap();
                }

                let uid = user_obj.map(|u| u.uid).map_or(root_user, |u| u);
                let gid = group_obj
                    .map(|g| g.gid)
                    .or_else(|| group_name_or_id.map(|g| Gid::from_raw(g.parse::<u32>().unwrap())))
                    .map_or(root_group, |g| g);

                (uid, gid)
            }
            None => (root_user, root_group),
        },
        None => (root_user, root_group),
    }
}

fn build_linux(image: Image) -> Linux {
    let host_uid = unistd::getuid();
    let host_gid = unistd::getgid();

    let image_user = resolve_user(image);
    Linux {
        uid_mappings: Some(vec![LinuxIdMapping {
            container_id: image_user.0.into(),
            host_id: host_uid.into(),
            size: 1,
        }]),
        gid_mappings: Some(vec![LinuxIdMapping {
            container_id: image_user.1.into(),
            host_id: host_gid.into(),
            size: 1,
        }]),
        namespaces: Some(vec![
            LinuxNamespace {
                typ: LinuxNamespaceType::Mount,
                path: None,
            },
            LinuxNamespace {
                typ: LinuxNamespaceType::Uts,
                path: None,
            },
            LinuxNamespace {
                typ: LinuxNamespaceType::Ipc,
                path: None,
            },
            LinuxNamespace {
                typ: LinuxNamespaceType::User,
                path: None,
            },
            LinuxNamespace {
                typ: LinuxNamespaceType::Pid,
                path: None,
            },
        ]),
        ..Default::default()
    }
}

fn build_mounts(
    lower_dirs: Vec<PathBuf>,
    upper_dir: PathBuf,
    work_dir: PathBuf,
    bin_dir: PathBuf,
) -> Vec<Mount> {
    let mounts = vec![
        Mount {
            destination: "/".into(),
            typ: Some("overlay".into()),
            source: Some("overlay".into()),
            options: Some(vec![
                overlay_option("lowerdir", &lower_dirs),
                overlay_option("upperdir", &[upper_dir]),
                overlay_option("workdir", &[work_dir]),
            ]),
        },
        Mount {
            destination: "/proc".into(),
            typ: Some("proc".into()),
            source: Some("proc".into()),
            options: None,
        },
        Mount {
            destination: "/dev".into(),
            typ: Some("tmpfs".into()),
            source: Some("tmpfs".into()),
            options: Some(vec![
                "nosuid".into(),
                "strictatime".into(),
                "mode=755".into(),
                "size=65536k".into(),
            ]),
        },
        Mount {
            destination: "/dev/pts".into(),
            typ: Some("devpts".into()),
            source: Some("devpts".into()),
            options: Some(vec![
                "nosuid".into(),
                "noexec".into(),
                "newinstance".into(),
                "ptmxmode=0666".into(),
                "mode=0620".into(),
            ]),
        },
        Mount {
            destination: "/dev/shm".into(),
            typ: Some("tmpfs".into()),
            source: Some("shm".into()),
            options: Some(vec![
                "nosuid".into(),
                "noexec".into(),
                "nodev".into(),
                "nodev".into(),
                "mode=1777".into(),
                "size=65536k".into(),
            ]),
        },
        Mount {
            destination: "/dev/mqueue".into(),
            typ: Some("mqueue".into()),
            source: Some("mqueue".into()),
            options: Some(vec!["nosuid".into(), "noexec".into(), "nodev".into()]),
        },
        Mount {
            destination: "/sys".into(),
            typ: Some("none".into()),
            source: Some("/sys".into()),
            options: Some(vec![
                "rbind".into(),
                "nosuid".into(),
                "noexec".into(),
                "nodev".into(),
                "ro".into(),
            ]),
        },
        Mount {
            destination: "/sys/fs/cgroup".into(),
            typ: Some("cgroup".into()),
            source: Some("cgroup".into()),
            options: Some(vec![
                "nosuid".into(),
                "noexec".into(),
                "nodev".into(),
                "relatime".into(),
                "ro".into(),
            ]),
        },
        Mount {
            destination: "/usr/bin/doe".into(),
            typ: Some("bind".into()),
            source: bin_dir.into(),
            options: Some(vec!["rbind".into(), "rw".into()]),
        },
    ];
    mounts
}

// TODO cleanup this clone mess, clone is required, because we need owned values, but could be improved I think.
fn build_cmd<I>(image: Image, config: ContainerConfig, arguments: I) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    let image_config = image.config;
    let image_entrypoint = image_config
        .as_ref()
        .map(|ic| ic.entrypoint.clone())
        .flatten();
    let entrypoint = config.entrypoint.or(image_entrypoint);

    let mut actual_cmd = Vec::new();

    if let Some(entrypoint) = entrypoint {
        actual_cmd.extend(entrypoint);
    }

    let image_cmd = image_config.map(|image_config| image_config.cmd).flatten();
    let cmd = config.cmd.or(image_cmd);

    if let Some(cmd) = cmd {
        actual_cmd.extend(cmd);
    }

    if actual_cmd.is_empty() {
        actual_cmd.push("sh".to_string());
    }

    actual_cmd.extend(arguments);

    actual_cmd
}

fn build_env(image: Image, config: ContainerConfig) -> Vec<String> {
    // Convert image keys=value to hashmap
    let mut map = HashMap::new();
    if let Some(envs) = image.config.map(|c| c.env).flatten() {
        for env in envs {
            let mut parts = env.splitn(2, '=');
            map.insert(
                parts.next().unwrap().to_string(),
                parts.next().unwrap().to_string(),
            );
        }
    }

    if let Some(envs) = config.env {
        map.extend(envs);
    }

    // TODO add inherted envvars from calling process

    let new_path = match map.get("PATH") {
        Some(current_path) => format!("{}:{}", current_path, BIN_DIR),
        None => BIN_DIR.to_string(),
    };

    map.insert("PATH".to_string(), new_path);

    map.into_iter()
        .map(|env| format!("{}={}", env.0, env.1))
        .collect()
}

fn build_rootless_runtime_bundle<I>(
    image: &Image,
    config: &ContainerConfig,
    mounts: Vec<Mount>,
    arguments: I,
) -> Spec
where
    I: IntoIterator<Item = String>,
{
    let root = Root {
        readonly: Some(false),
        ..Default::default()
    };

    // TODO overuse of clone, make this more efficient?
    let process = Process {
        args: build_cmd(image.clone(), config.clone(), arguments).into(),
        env: build_env(image.clone(), config.clone()).into(),
        ..Default::default()
    };

    Spec {
        version: "1.0.2-dev".into(),
        root: Some(root),
        mounts: Some(mounts),
        process: Some(process),
        hostname: None,
        hooks: None,
        annotations: None,
        linux: Some(build_linux(image.clone())),
        solaris: None,
        windows: None,
        vm: None,
    }
}

fn write_container_script(path: &Path, target: &str) -> Result<()> {
    let script = format!("#!{}/doe call\n{}\n", BIN_DIR, target);

    let mut file =
        File::create(&path).with_context(|| format!("could not create file {:?}", &path))?;

    file.write_all(script.as_bytes())
        .with_context(|| format!("could not write to file {:?}", &file))?;

    let mut permissions = fs::metadata(&path)
        .with_context(|| format!("could not read metadata for file {:?}", &path))?
        .permissions();
    permissions.set_mode(0o744);

    fs::set_permissions(&path, permissions).with_context(|| {
        format!(
            "could permissions to `{}` for file `{}`",
            744,
            path.to_str().unwrap()
        )
    })?;

    Ok(())
}

#[async_trait]
impl RuntimeBundleGenerator for RunGenerator {
    async fn build<S, I>(
        &self,
        container_name: S,
        config: &ContainerConfig,
        arguments: I,
    ) -> Result<PathBuf>
    where
        S: Into<String> + Send,
        I: IntoIterator<Item = String> + Send,
    {
        let container: String = container_name.into();
        let mut manager = ImageManager::default();
        let image = manager.prepare(&config.image).await?;

        let lower_dirs: Vec<PathBuf> = image
            .rootfs
            .diff_ids
            .iter()
            .map(|diff_id| {
                let mut layer_dir = self.layer_dir.clone();
                layer_dir.push(diff_id.algorithm.to_string());
                layer_dir.push(diff_id.encoded.to_string());
                layer_dir
            })
            .collect();

        // create_directories(&lower_dirs)?;

        let upper_dir = self.location(&container, "upper");
        let work_dir = self.location(&container, "work");
        let bin_dir = self.location(&container, "bin");
        let root_fs = self.location(&container, "rootfs");
        let config_file = self.location(&container, "config.json");
        create_directories(&[
            upper_dir.clone(),
            work_dir.clone(),
            bin_dir.clone(),
            root_fs,
        ])?;

        // Add linked containers to bin directory
        if let Some(links) = &config.links {
            for (name, container) in links {
                let mut script_path = bin_dir.clone();
                script_path.push(name);

                log::debug!(
                    "creating binary `{}` linked to container `{}` at `{}`",
                    name,
                    container,
                    script_path.to_str().unwrap()
                );
                write_container_script(&script_path, &container)?;
            }
        };

        // Add current binary to bin directory
        let mut doe_path = bin_dir.clone();
        doe_path.push("doe");
        let current = env::current_exe()?;
        fs::hard_link(current, doe_path)?;

        let mounts = build_mounts(lower_dirs, upper_dir, work_dir, bin_dir);

        let spec = build_rootless_runtime_bundle(&image, config, mounts, arguments);
        spec.save(&config_file)?;

        let container_dir = self.container_dir(&container);
        Ok(container_dir)

        // 7. Generate scripts for container's links
        // 8. Mount scripts and current binary
        // 9. Overwrite with any user defined configuration
    }
}

pub struct CallGenerator {}

impl Default for CallGenerator {
    fn default() -> Self {
        CallGenerator {}
    }
}
