//! The saved server list (`servers.toml` in the data folder).

use atlas_shared::fs_safety::atomic_write;
use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Server {
    pub id: String,
    pub name: String,
    pub address: String,
    pub port: u16,
    #[serde(default)]
    pub favorite: bool,
    pub last_played: Option<u64>,
    /// From the last successful ping.
    pub version: Option<String>,
    /// PNG as base64 (what the server sends), from the last ping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

impl Server {
    pub fn host_port(&self) -> String {
        if self.port == 25565 { self.address.clone() } else { format!("{}:{}", self.address, self.port) }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    servers: Vec<Server>,
}

pub struct Servers {
    file: PathBuf,
}

/// "host", "host:port", "[v6]:port" -> (host, port)
pub fn parse_address(input: &str) -> AtlasResult<(String, u16)> {
    let s = input.trim();
    let bad = || AtlasError::new(ErrorCode::InvalidArgument, format!("'{input}' isn't a valid server address.")).suggest("Use host or host:port, e.g. play.example.net:25565");
    if s.is_empty() || s.contains(char::is_whitespace) || s.contains('/') {
        return Err(bad());
    }
    if let Some(rest) = s.strip_prefix('[') {
        let (h, after) = rest.split_once(']').ok_or_else(bad)?;
        let port = match after.strip_prefix(':') {
            Some(p) => p.parse().map_err(|_| bad())?,
            None if after.is_empty() => 25565,
            None => return Err(bad()),
        };
        return Ok((h.to_string(), port));
    }
    match s.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => Ok((h.to_string(), p.parse().map_err(|_| bad())?)),
        Some(_) => Ok((s.to_string(), 25565)), // bare IPv6
        None => Ok((s.to_string(), 25565)),
    }
}

impl Servers {
    pub fn new(data_root: &std::path::Path) -> Self {
        Self { file: data_root.join("servers.toml") }
    }

    fn read(&self) -> AtlasResult<File> {
        match std::fs::read_to_string(&self.file) {
            Ok(t) => toml::from_str(&t).map_err(|e| AtlasError::new(ErrorCode::ConfigInvalid, "The server list is damaged.").details(e).suggest("Fix or delete servers.toml.")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(File::default()),
            Err(e) => Err(AtlasError::io(ErrorCode::ConfigIo, "read the server list", &self.file, e)),
        }
    }

    fn write(&self, f: &File) -> AtlasResult<()> {
        let t = toml::to_string_pretty(f).map_err(|e| AtlasError::new(ErrorCode::Internal, "Couldn't save servers.").details(e))?;
        atomic_write(&self.file, t.as_bytes())
    }

    /// Favourites first, then most recently played.
    pub fn list(&self) -> AtlasResult<Vec<Server>> {
        let mut v = self.read()?.servers;
        v.sort_by(|a, b| b.favorite.cmp(&a.favorite).then(b.last_played.unwrap_or(0).cmp(&a.last_played.unwrap_or(0))).then(a.name.cmp(&b.name)));
        Ok(v)
    }

    pub fn add(&self, name: &str, address: &str) -> AtlasResult<Server> {
        let (host, port) = parse_address(address)?;
        let name = name.trim();
        let s = Server {
            id: uuid::Uuid::new_v4().simple().to_string()[..10].to_string(),
            name: if name.is_empty() { host.clone() } else { name.chars().take(64).collect() },
            address: host,
            port,
            favorite: false,
            last_played: None,
            version: None,
            icon: None,
        };
        let mut f = self.read()?;
        f.servers.push(s.clone());
        self.write(&f)?;
        Ok(s)
    }

    pub fn update(&self, id: &str, edit: impl FnOnce(&mut Server) -> AtlasResult<()>) -> AtlasResult<Server> {
        let mut f = self.read()?;
        let s = f.servers.iter_mut().find(|s| s.id == id).ok_or_else(|| AtlasError::new(ErrorCode::InvalidArgument, "No such server."))?;
        edit(s)?;
        let out = s.clone();
        self.write(&f)?;
        Ok(out)
    }

    pub fn edit(&self, id: &str, name: Option<&str>, address: Option<&str>) -> AtlasResult<Server> {
        let addr = address.map(parse_address).transpose()?;
        self.update(id, |s| {
            if let Some(n) = name.map(str::trim).filter(|n| !n.is_empty()) {
                s.name = n.chars().take(64).collect();
            }
            if let Some((h, p)) = addr {
                s.address = h;
                s.port = p;
                s.version = None;
                s.icon = None;
            }
            Ok(())
        })
    }

    pub fn remove(&self, id: &str) -> AtlasResult<()> {
        let mut f = self.read()?;
        let n = f.servers.len();
        f.servers.retain(|s| s.id != id);
        if n == f.servers.len() {
            return Err(AtlasError::new(ErrorCode::InvalidArgument, "No such server."));
        }
        self.write(&f)
    }

    pub fn get(&self, id: &str) -> AtlasResult<Server> {
        self.read()?.servers.into_iter().find(|s| s.id == id).ok_or_else(|| AtlasError::new(ErrorCode::InvalidArgument, "No such server."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        assert_eq!(parse_address("play.example.net").unwrap(), ("play.example.net".into(), 25565));
        assert_eq!(parse_address("1.2.3.4:25570").unwrap(), ("1.2.3.4".into(), 25570));
        assert_eq!(parse_address("[::1]:25566").unwrap(), ("::1".into(), 25566));
        assert_eq!(parse_address("::1").unwrap(), ("::1".into(), 25565));
        for bad in ["", "a b", "host:99999", "host:abc", "http://x"] {
            assert!(parse_address(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn crud_and_ordering() {
        let t = tempfile::tempdir().unwrap();
        let s = Servers::new(t.path());
        let a = s.add("Alpha", "a.example.net").unwrap();
        let b = s.add("", "b.example.net:25570").unwrap();
        assert_eq!(b.name, "b.example.net");
        s.update(&b.id, |x| {
            x.favorite = true;
            Ok(())
        })
        .unwrap();
        assert_eq!(s.list().unwrap()[0].id, b.id, "favourites first");
        let a = s.edit(&a.id, Some("Renamed"), Some("c.example.net:1")).unwrap();
        assert_eq!((a.name.as_str(), a.port), ("Renamed", 1));
        s.remove(&a.id).unwrap();
        assert_eq!(s.list().unwrap().len(), 1);
        assert!(s.remove("nope").is_err());
    }
}
