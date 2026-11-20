//! `kit::host`: the machine's name and addresses.

use cupertino_kit::host;
use std::net::Ipv4Addr;

#[test]
fn the_host_name_is_the_kernels() {
    let kernel = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap();
    let kernel = kernel.trim_end_matches('\n');
    assert_eq!(
        host::name().as_deref(),
        (!kernel.is_empty()).then_some(kernel)
    );
}

#[test]
fn the_addresses_include_the_loopback_one() {
    let addresses = host::ipv4_addresses().unwrap();
    assert!(addresses.contains(&Ipv4Addr::LOCALHOST), "{addresses:?}");
}
