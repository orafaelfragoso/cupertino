//! The resolver configuration: a `resolv.conf` written from the DHCP servers. There is no
//! caching resolver and nothing links `/etc/resolv.conf` here; the host points it at this file.
//! IPv6 resolver advertisements (RDNSS) are not read: the kernel hands them to user space only
//! through a netlink option message this stack does not parse, so IPv6-only networks need a
//! DHCPv4 server or a manual file.

use std::fs;
use std::io::Write;
use std::net::Ipv4Addr;
use std::path::Path;

use crate::error::{Error, Result};

/// Where the file goes unless the host says otherwise.
pub const DEFAULT_PATH: &str = "/run/cupertino/resolv.conf";

/// The most nameservers the C library reads (`MAXNS` in resolv.conf(5)); more would be ignored.
const MAX_NAMESERVERS: usize = 3;

/// The text of the file: a search domain when there is one, then the nameservers in the order the
/// server gave them.
pub fn render(servers: &[Ipv4Addr], domain: Option<&str>) -> String {
    let mut text = String::from("# written by cupertino-netd\n");
    if let Some(domain) = domain {
        text.push_str(&format!("search {domain}\n"));
    }
    for server in servers.iter().take(MAX_NAMESERVERS) {
        text.push_str(&format!("nameserver {server}\n"));
    }
    text
}

/// Replaces the file at `path` with the configuration: written next to it, then renamed, so a
/// reader sees the old file or the new one and never half of one. Creates the directory.
pub fn write(path: &Path, servers: &[Ipv4Addr], domain: Option<&str>) -> Result<()> {
    let directory = path.parent().filter(|directory| !directory.as_os_str().is_empty()).ok_or_else(|| Error::Io {
        path: path.to_owned(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "the path has no directory"),
    })?;
    fs::create_dir_all(directory).map_err(Error::io(directory))?;
    let mut file = tempfile::NamedTempFile::new_in(directory).map_err(Error::io(directory))?;
    file.write_all(render(servers, domain).as_bytes()).map_err(Error::io(path))?;
    // 0644: every process resolves names
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file().set_permissions(fs::Permissions::from_mode(0o644)).map_err(Error::io(path))?;
    }
    file.persist(path).map_err(|error| Error::Io { path: path.to_owned(), source: error.error })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servers_keep_their_order_and_the_domain_becomes_the_search_list() {
        let text = render(&[Ipv4Addr::new(10, 0, 0, 2), Ipv4Addr::new(1, 1, 1, 1)], Some("lan.example"));
        assert_eq!(text, "# written by cupertino-netd\nsearch lan.example\nnameserver 10.0.0.2\nnameserver 1.1.1.1\n");
    }

    #[test]
    fn only_the_first_three_servers_are_written() {
        let servers: Vec<_> = (1..=5).map(|n| Ipv4Addr::new(10, 0, 0, n)).collect();
        let text = render(&servers, None);
        assert_eq!(text, "# written by cupertino-netd\nnameserver 10.0.0.1\nnameserver 10.0.0.2\nnameserver 10.0.0.3\n");
    }

    #[test]
    fn writing_replaces_the_file_and_creates_the_directory() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("run/cupertino/resolv.conf");
        write(&path, &[Ipv4Addr::new(10, 0, 0, 1)], None).unwrap();
        write(&path, &[Ipv4Addr::new(10, 0, 1, 1)], Some("home")).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "# written by cupertino-netd\nsearch home\nnameserver 10.0.1.1\n");
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }
}
