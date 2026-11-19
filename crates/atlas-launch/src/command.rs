use atlas_shared::fs_safety::safe_join;
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use atlas_versions::rules::{allowed_with, Platform, Rule};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Who is playing. `access_token` never gets logged (redaction catches it too).
#[derive(Clone)]
pub struct LaunchAuth {
    pub name: String,
    pub uuid: String,
    pub access_token: String,
    pub xuid: String,
    pub demo: bool,
}

impl std::fmt::Debug for LaunchAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaunchAuth").field("name", &self.name).field("uuid", &self.uuid).field("demo", &self.demo).finish_non_exhaustive()
    }
}

impl LaunchAuth {
    /// Demo mode, what Mojang's own launcher does for accounts without the game.
    pub fn demo() -> Self {
        Self { name: "Player".into(), uuid: "00000000000000000000000000000000".into(), access_token: "0".into(), xuid: "0".into(), demo: true }
    }
}

/// Everything needed to build the command line.
#[derive(Debug, Clone)]
pub struct LaunchSpec {
    pub data_root: PathBuf,
    pub version_id: String,
    pub game_dir: PathBuf,
    pub natives_dir: PathBuf,
    pub java: PathBuf,
    pub min_mb: u32,
    pub max_mb: u32,
    pub extra_jvm: Vec<String>,
    pub extra_game: Vec<String>,
    pub resolution: Option<(u32, u32)>,
    pub fullscreen: bool,
    /// "host" or "host:port" to join right away.
    pub server: Option<String>,
    pub auth: LaunchAuth,
    pub platform: Platform,
}

#[derive(Debug, Clone)]
pub struct LaunchCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// Native jars that still need unpacking into `natives_dir` (old versions).
    pub native_jars: Vec<PathBuf>,
}

fn broken(msg: impl Into<String>) -> AtlasError {
    AtlasError::new(ErrorCode::InvalidInstallation, "The installed version files are incomplete or damaged.")
        .details(msg.into())
        .suggest("Repair the version on the Versions page and try again.")
}

fn rules_of(v: &Value) -> Vec<Rule> {
    v.get("rules").and_then(|r| serde_json::from_value(r.clone()).ok()).unwrap_or_default()
}

/// Mojang argument lists mix plain strings and `{rules, value}` objects.
fn collect_args(list: &[Value], p: Platform, features: &[(&str, bool)], out: &mut Vec<String>) {
    for item in list {
        match item {
            Value::String(s) => out.push(s.clone()),
            Value::Object(_) => {
                if !allowed_with(&rules_of(item), p, features) {
                    continue;
                }
                match item.get("value") {
                    Some(Value::String(s)) => out.push(s.clone()),
                    Some(Value::Array(a)) => out.extend(a.iter().filter_map(|x| x.as_str().map(str::to_owned))),
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn substitute(arg: &str, vars: &HashMap<&str, String>) -> String {
    let mut out = String::with_capacity(arg.len());
    let mut rest = arg;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) => {
                let key = &after[..end];
                match vars.get(key) {
                    Some(v) => out.push_str(v),
                    None => out.push_str(&rest[start..start + 2 + end + 1]),
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn split_server(s: &str) -> (String, String) {
    match s.rsplit_once(':') {
        Some((h, p)) if p.parse::<u16>().is_ok() => (h.to_string(), p.to_string()),
        _ => (s.to_string(), "25565".into()),
    }
}

fn read_json(root: &Path, id: &str) -> AtlasResult<Value> {
    let p = root.join("versions").join(id).join(format!("{id}.json"));
    let raw = std::fs::read(&p).map_err(|e| broken(format!("{}: {e}", p.display())))?;
    serde_json::from_slice(&raw).map_err(|e| broken(e.to_string()))
}

fn lib_key(l: &Value) -> Option<String> {
    // "group:artifact" (plus classifier) so a loader's newer asm replaces vanilla's
    let name = l.get("name")?.as_str()?;
    let parts: Vec<&str> = name.split(':').collect();
    Some(match parts.as_slice() {
        [g, a, _v, c, ..] => format!("{g}:{a}:{c}"),
        [g, a, ..] => format!("{g}:{a}"),
        _ => name.to_string(),
    })
}

/// Load a version file, merging it onto its parent when it's a loader
/// profile (`inheritsFrom`). Returns the merged file and the game jar.
pub fn resolve_version(root: &Path, id: &str) -> AtlasResult<(Value, PathBuf)> {
    let child = read_json(root, id)?;
    let Some(parent_id) = child.get("inheritsFrom").and_then(|p| p.as_str()).map(str::to_owned) else {
        let jar = root.join("versions").join(id).join(format!("{id}.jar"));
        return Ok((child, jar));
    };
    let mut merged = read_json(root, &parent_id)?;
    let child_libs: Vec<Value> = child.get("libraries").and_then(|l| l.as_array()).cloned().unwrap_or_default();
    let taken: std::collections::HashSet<String> = child_libs.iter().filter_map(lib_key).collect();
    let mut libs = child_libs;
    for l in merged.get("libraries").and_then(|l| l.as_array()).cloned().unwrap_or_default() {
        if lib_key(&l).is_none_or(|k| !taken.contains(&k)) {
            libs.push(l);
        }
    }
    let m = merged.as_object_mut().ok_or_else(|| broken("parent version file isn't an object"))?;
    m.insert("libraries".into(), Value::Array(libs));
    m.insert("id".into(), Value::String(id.to_string()));
    if let Some(mc) = child.get("mainClass") {
        m.insert("mainClass".into(), mc.clone());
    }
    if let Some(legacy) = child.get("minecraftArguments") {
        m.insert("minecraftArguments".into(), legacy.clone());
    }
    if let Some(ca) = child.get("arguments") {
        let mut args = m.get("arguments").cloned().unwrap_or(serde_json::json!({}));
        for side in ["jvm", "game"] {
            let mut list = args.get(side).and_then(|a| a.as_array()).cloned().unwrap_or_default();
            list.extend(ca.get(side).and_then(|a| a.as_array()).cloned().unwrap_or_default());
            args[side] = Value::Array(list);
        }
        m.insert("arguments".into(), args);
    }
    let own = child.get("atlasOwnJar").and_then(|b| b.as_bool()).unwrap_or(false);
    let jar_id = if own { id } else { parent_id.as_str() };
    let jar = root.join("versions").join(jar_id).join(format!("{jar_id}.jar"));
    Ok((merged, jar))
}

pub fn build_command(spec: &LaunchSpec) -> AtlasResult<LaunchCommand> {
    let (v, client) = resolve_version(&spec.data_root, &spec.version_id)?;
    let raw = serde_json::to_vec(&v).unwrap_or_default();
    let p = spec.platform;

    // classpath: allowed library artifacts + the client jar
    let libs_dir = spec.data_root.join("libraries");
    let mut cp: Vec<PathBuf> = vec![];
    let mut native_jars = vec![];
    for lib in v.get("libraries").and_then(|l| l.as_array()).into_iter().flatten() {
        if !allowed_with(&rules_of(lib), p, &[]) {
            continue;
        }
        let dl = lib.get("downloads");
        if let Some(path) = dl.and_then(|d| d.get("artifact")).and_then(|a| a.get("path")).and_then(|x| x.as_str()) {
            let jar = safe_join(&libs_dir, path)?;
            if !jar.is_file() {
                return Err(broken(format!("missing library {path}")));
            }
            if !cp.contains(&jar) {
                cp.push(jar);
            }
        }
        if let Some(key) = lib.get("natives").and_then(|n| n.get(p.os)).and_then(|k| k.as_str()) {
            let key = key.replace("${arch}", p.bits());
            if let Some(path) = dl.and_then(|d| d.get("classifiers")).and_then(|c| c.get(&key)).and_then(|a| a.get("path")).and_then(|x| x.as_str()) {
                native_jars.push(safe_join(&libs_dir, path)?);
            }
        }
    }
    if !client.is_file() {
        return Err(broken(format!("missing {}", client.display())));
    }
    cp.push(client);
    let sep = if p.os == "windows" { ";" } else { ":" };
    let classpath = cp.iter().map(|x| x.to_string_lossy().into_owned()).collect::<Vec<_>>().join(sep);

    let asset_index = v.get("assetIndex").and_then(|a| a.get("id")).and_then(|x| x.as_str()).unwrap_or("legacy").to_string();
    let assets_root = spec.data_root.join("assets");
    let version_type = v.get("type").and_then(|x| x.as_str()).unwrap_or("release").to_string();
    let (w, h) = spec.resolution.unwrap_or((854, 480));

    let mut vars: HashMap<&str, String> = HashMap::new();
    vars.insert("auth_player_name", spec.auth.name.clone());
    vars.insert("auth_uuid", spec.auth.uuid.clone());
    vars.insert("auth_access_token", spec.auth.access_token.clone());
    vars.insert("auth_session", format!("token:{}", spec.auth.access_token));
    vars.insert("auth_xuid", spec.auth.xuid.clone());
    vars.insert("clientid", String::new());
    vars.insert("user_type", "msa".into());
    vars.insert("user_properties", "{}".into());
    vars.insert("version_name", spec.version_id.clone());
    vars.insert("version_type", version_type);
    vars.insert("game_directory", spec.game_dir.to_string_lossy().into_owned());
    vars.insert("assets_root", assets_root.to_string_lossy().into_owned());
    vars.insert("game_assets", assets_root.join("virtual/legacy").to_string_lossy().into_owned());
    vars.insert("assets_index_name", asset_index);
    vars.insert("natives_directory", spec.natives_dir.to_string_lossy().into_owned());
    vars.insert("library_directory", libs_dir.to_string_lossy().into_owned());
    vars.insert("classpath_separator", sep.into());
    vars.insert("classpath", classpath);
    vars.insert("launcher_name", "atlas-launcher".into());
    vars.insert("launcher_version", atlas_shared::ATLAS_VERSION.into());
    vars.insert("resolution_width", w.to_string());
    vars.insert("resolution_height", h.to_string());
    if let Some(s) = &spec.server {
        vars.insert("quickPlayMultiplayer", s.clone());
    }

    let has_quickplay = String::from_utf8_lossy(&raw).contains("quickPlayMultiplayer");
    let features: Vec<(&str, bool)> = vec![
        ("is_demo_user", spec.auth.demo),
        ("has_custom_resolution", spec.resolution.is_some()),
        ("is_quick_play_multiplayer", spec.server.is_some() && has_quickplay),
    ];

    let mut jvm = vec![format!("-Xms{}M", spec.min_mb), format!("-Xmx{}M", spec.max_mb)];
    jvm.extend(spec.extra_jvm.iter().cloned());
    let mut game = vec![];

    if let Some(args) = v.get("arguments") {
        let mut j = vec![];
        collect_args(args.get("jvm").and_then(|x| x.as_array()).map(Vec::as_slice).unwrap_or(&[]), p, &features, &mut j);
        jvm.extend(j);
        collect_args(args.get("game").and_then(|x| x.as_array()).map(Vec::as_slice).unwrap_or(&[]), p, &features, &mut game);
    } else if let Some(legacy) = v.get("minecraftArguments").and_then(|x| x.as_str()) {
        // pre-1.13 versions: one space-separated string, and we add the jvm bits ourselves
        jvm.push("-Djava.library.path=${natives_directory}".into());
        if p.os == "osx" {
            jvm.push("-XstartOnFirstThread".into());
        }
        jvm.push("-cp".into());
        jvm.push("${classpath}".into());
        game.extend(legacy.split_whitespace().map(str::to_owned));
        if spec.auth.demo {
            game.push("--demo".into());
        }
        if spec.resolution.is_some() {
            game.extend(["--width".into(), "${resolution_width}".into(), "--height".into(), "${resolution_height}".into()]);
        }
    } else {
        return Err(broken("version file has no launch arguments"));
    }
    if spec.server.is_some() && !has_quickplay {
        let (host, port) = split_server(spec.server.as_deref().unwrap_or_default());
        game.extend(["--server".into(), host, "--port".into(), port]);
    }
    if spec.fullscreen {
        game.push("--fullscreen".into());
    }
    game.extend(spec.extra_game.iter().cloned());

    let main_class = v.get("mainClass").and_then(|x| x.as_str()).ok_or_else(|| broken("no mainClass"))?;
    let mut args: Vec<String> = jvm.iter().map(|a| substitute(a, &vars)).collect();
    args.push(main_class.to_string());
    args.extend(game.iter().map(|a| substitute(a, &vars)));
    // anything still in ${...} form is an unknown feature arg; drop it and its flag
    let mut cleaned = Vec::with_capacity(args.len());
    for a in args {
        if a.contains("${") {
            if cleaned.last().is_some_and(|l: &String| l.starts_with("--")) {
                cleaned.pop();
            }
            continue;
        }
        cleaned.push(a);
    }

    Ok(LaunchCommand { program: spec.java.clone(), args: cleaned, cwd: spec.game_dir.clone(), native_jars })
}

/// Command line for logs, with the token blanked out.
pub fn display_command(c: &LaunchCommand) -> String {
    let mut parts = vec![c.program.display().to_string()];
    let mut hide_next = false;
    for a in &c.args {
        if hide_next {
            parts.push("[REDACTED]".into());
            hide_next = false;
            continue;
        }
        hide_next = a == "--accessToken" || a == "--session";
        parts.push(a.clone());
    }
    parts.join(" ")
}

pub fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn setup(json: Value) -> (tempfile::TempDir, LaunchSpec) {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().to_path_buf();
        let vd = root.join("versions/X");
        std::fs::create_dir_all(&vd).unwrap();
        std::fs::write(vd.join("X.json"), serde_json::to_vec(&json).unwrap()).unwrap();
        std::fs::write(vd.join("X.jar"), b"jar").unwrap();
        let lib = root.join("libraries/a/b.jar");
        std::fs::create_dir_all(lib.parent().unwrap()).unwrap();
        std::fs::write(&lib, b"lib").unwrap();
        let spec = LaunchSpec {
            data_root: root.clone(),
            version_id: "X".into(),
            game_dir: root.join("game"),
            natives_dir: root.join("natives"),
            java: "java".into(),
            min_mb: 1024,
            max_mb: 2048,
            extra_jvm: vec!["-Dfoo=bar".into()],
            extra_game: vec![],
            resolution: None,
            fullscreen: false,
            server: None,
            auth: LaunchAuth { name: "Steve".into(), uuid: "u".into(), access_token: "SECRET".into(), xuid: "x".into(), demo: false },
            platform: Platform { os: "linux", arch: "x86_64" },
        };
        (t, spec)
    }

    fn modern() -> Value {
        json!({
            "id": "X", "type": "release", "mainClass": "net.minecraft.client.main.Main",
            "assetIndex": {"id": "29"},
            "libraries": [
                {"name": "a:b:1", "downloads": {"artifact": {"path": "a/b.jar"}}},
                {"name": "w:w:1", "rules": [{"action": "allow", "os": {"name": "windows"}}], "downloads": {"artifact": {"path": "w/w.jar"}}}
            ],
            "arguments": {
                "jvm": [{"rules": [{"action": "allow", "os": {"name": "osx"}}], "value": ["-XstartOnFirstThread"]}, "-Djava.library.path=${natives_directory}", "-cp", "${classpath}"],
                "game": ["--username", "${auth_player_name}", "--accessToken", "${auth_access_token}", "--assetIndex", "${assets_index_name}",
                    {"rules": [{"action": "allow", "features": {"is_demo_user": true}}], "value": "--demo"},
                    {"rules": [{"action": "allow", "features": {"has_custom_resolution": true}}], "value": ["--width", "${resolution_width}", "--height", "${resolution_height}"]},
                    {"rules": [{"action": "allow", "features": {"is_quick_play_multiplayer": true}}], "value": ["--quickPlayMultiplayer", "${quickPlayMultiplayer}"]},
                    "--clientId", "${clientid}", "--weird", "${not_a_thing}"]
            }
        })
    }

    #[test]
    fn modern_arguments() {
        let (_t, spec) = setup(modern());
        let c = build_command(&spec).unwrap();
        let a = c.args.join(" ");
        assert!(a.starts_with("-Xms1024M -Xmx2048M -Dfoo=bar"), "{a}");
        assert!(!a.contains("-XstartOnFirstThread"), "osx-only arg skipped on linux");
        assert!(a.contains("--username Steve"));
        assert!(a.contains("--assetIndex 29"));
        assert!(!a.contains("--demo"));
        assert!(!a.contains("--width"));
        assert!(!a.contains("--weird"), "unknown placeholder dropped with its flag: {a}");
        let cp_i = c.args.iter().position(|x| x == "-cp").unwrap();
        // compare with forward slashes so this also passes on Windows
        let cp = c.args[cp_i + 1].replace('\\', "/");
        assert!(cp.contains("a/b.jar") && cp.ends_with("X.jar"), "{cp}");
        assert!(!cp.contains("w/w.jar"));
        let shown = display_command(&c);
        assert!(!shown.contains("SECRET") && shown.contains("[REDACTED]"));
    }

    #[test]
    fn demo_resolution_and_server() {
        let (_t, mut spec) = setup(modern());
        spec.auth = LaunchAuth::demo();
        spec.resolution = Some((1600, 900));
        spec.server = Some("play.example.net:25570".into());
        let a = build_command(&spec).unwrap().args.join(" ");
        assert!(a.contains("--demo"));
        assert!(a.contains("--width 1600 --height 900"));
        assert!(a.contains("--quickPlayMultiplayer play.example.net:25570"));
    }

    #[test]
    fn legacy_arguments_and_server() {
        let (_t, mut spec) = setup(json!({
            "id": "X", "type": "release", "mainClass": "net.minecraft.client.main.Main",
            "assetIndex": {"id": "1.8"},
            "libraries": [{"name": "a:b:1", "downloads": {"artifact": {"path": "a/b.jar"}}}],
            "minecraftArguments": "--username ${auth_player_name} --session ${auth_session} --gameDir ${game_directory}"
        }));
        spec.server = Some("mc.example.org".into());
        let c = build_command(&spec).unwrap();
        let a = c.args.join(" ");
        assert!(a.contains("-Djava.library.path="));
        assert!(a.contains("--session token:SECRET"));
        assert!(a.contains("--server mc.example.org --port 25565"));
        assert!(!display_command(&c).contains("SECRET"));
    }

    #[test]
    fn loader_profile_merges_with_parent() {
        let (t, mut spec) = setup(modern());
        let pd = t.path().join("versions/fabric-x");
        std::fs::create_dir_all(&pd).unwrap();
        let lib = t.path().join("libraries/f/l.jar");
        std::fs::create_dir_all(lib.parent().unwrap()).unwrap();
        std::fs::write(&lib, b"loader").unwrap();
        std::fs::write(pd.join("fabric-x.json"), serde_json::to_vec(&json!({
            "id": "fabric-x", "inheritsFrom": "X", "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
            "arguments": {"jvm": ["-DFabricMcEmu= net.minecraft.client.main.Main "], "game": []},
            "libraries": [{"name": "net.fabricmc:loader:1", "downloads": {"artifact": {"path": "f/l.jar"}}}, {"name": "a:b:2", "downloads": {"artifact": {"path": "f/l.jar"}}}]
        })).unwrap()).unwrap();
        spec.version_id = "fabric-x".into();
        let c = build_command(&spec).unwrap();
        let a = c.args.join(" ");
        assert!(a.contains("net.fabricmc.loader.impl.launch.knot.KnotClient"));
        assert!(a.contains("-DFabricMcEmu="));
        assert!(a.contains("--username Steve"), "parent game args kept");
        let cp_i = c.args.iter().position(|x| x == "-cp").unwrap();
        let cp = c.args[cp_i + 1].replace('\\', "/");
        assert!(cp.starts_with(&lib.to_string_lossy().replace('\\', "/")), "loader libs first: {cp}");
        assert!(!cp.contains("a/b.jar"), "vanilla a:b replaced by the loader's a:b");
        assert!(cp.ends_with("versions/X/X.jar"), "parent jar used: {cp}");
    }

    #[test]
    fn missing_library_is_reported() {
        let (t, spec) = setup(modern());
        std::fs::remove_file(t.path().join("libraries/a/b.jar")).unwrap();
        assert_eq!(build_command(&spec).unwrap_err().code, ErrorCode::InvalidInstallation);
    }
}
