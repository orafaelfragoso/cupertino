//! The two Bluetooth socket address layouts the kernel uAPI defines and rustix does not wrap.
//!
//! rustix has no Bluetooth address types, but it lets a crate implement [`SocketAddrArg`] for its
//! own. That trait is `unsafe` to implement because the kernel reads the bytes; each impl below
//! hands rustix a pointer to a stack array of exactly the size and layout of the C struct, so this
//! file holds the only `unsafe` in the crate.
#![allow(unsafe_code)]

use rustix::net::addr::{SocketAddrArg, SocketAddrLen, SocketAddrOpaque};

use crate::address::{Address, AddressType};

/// `AF_BLUETOOTH` (`include/linux/socket.h`).
pub(crate) const AF_BLUETOOTH: u16 = 31;

/// The controller number that means "no controller" in `struct sockaddr_hci` (`HCI_DEV_NONE`,
/// `include/net/bluetooth/hci_sock.h`).
const HCI_DEV_NONE: u16 = 0xFFFF;

/// `HCI_CHANNEL_CONTROL`, the management channel (`include/net/bluetooth/hci_sock.h`).
const HCI_CHANNEL_CONTROL: u16 = 3;

/// The address of the management control channel: `struct sockaddr_hci` with `hci_dev` =
/// `HCI_DEV_NONE` and `hci_channel` = `HCI_CHANNEL_CONTROL`.
pub(crate) struct SocketAddrControl;

impl SocketAddrControl {
    /// The 6 bytes of `struct sockaddr_hci { u16 family; u16 dev; u16 channel; }`.
    pub(crate) fn to_bytes(&self) -> [u8; 6] {
        let mut out = [0u8; 6];
        out[0..2].copy_from_slice(&AF_BLUETOOTH.to_ne_bytes());
        out[2..4].copy_from_slice(&HCI_DEV_NONE.to_ne_bytes());
        out[4..6].copy_from_slice(&HCI_CHANNEL_CONTROL.to_ne_bytes());
        out
    }
}

// SAFETY: `with_sockaddr` passes a pointer to `bytes`, which is live for the call, with its exact
// length, and the bytes are `struct sockaddr_hci`.
unsafe impl SocketAddrArg for SocketAddrControl {
    unsafe fn with_sockaddr<R>(&self, f: impl FnOnce(*const SocketAddrOpaque, SocketAddrLen) -> R) -> R {
        let bytes = self.to_bytes();
        f(bytes.as_ptr().cast(), bytes.len() as SocketAddrLen)
    }
}

/// An L2CAP socket address: `struct sockaddr_l2` (`include/net/bluetooth/l2cap.h`).
pub(crate) struct SocketAddrL2 {
    pub(crate) psm: u16,
    pub(crate) address: Address,
    pub(crate) cid: u16,
    pub(crate) kind: AddressType,
}

impl SocketAddrL2 {
    /// The 14 bytes of `struct sockaddr_l2 { u16 family; le16 psm; bdaddr_t bdaddr; le16 cid; u8
    /// bdaddr_type; }` including its trailing pad byte. `bdaddr_t` is the address in wire order.
    pub(crate) fn to_bytes(&self) -> [u8; 14] {
        let mut out = [0u8; 14];
        out[0..2].copy_from_slice(&AF_BLUETOOTH.to_ne_bytes());
        out[2..4].copy_from_slice(&self.psm.to_le_bytes());
        out[4..10].copy_from_slice(&self.address.to_wire());
        out[10..12].copy_from_slice(&self.cid.to_le_bytes());
        out[12] = self.kind.to_wire();
        out
    }
}

// SAFETY: as for `SocketAddrControl`; the bytes are `struct sockaddr_l2`.
unsafe impl SocketAddrArg for SocketAddrL2 {
    unsafe fn with_sockaddr<R>(&self, f: impl FnOnce(*const SocketAddrOpaque, SocketAddrLen) -> R) -> R {
        let bytes = self.to_bytes();
        f(bytes.as_ptr().cast(), bytes.len() as SocketAddrLen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_control_channel_address_is_a_sockaddr_hci() {
        // family AF_BLUETOOTH (31), dev HCI_DEV_NONE (0xFFFF), channel HCI_CHANNEL_CONTROL (3),
        // each a native-endian u16 (the kernel reads them as C integers).
        let mut expected = Vec::new();
        expected.extend_from_slice(&31u16.to_ne_bytes());
        expected.extend_from_slice(&0xFFFFu16.to_ne_bytes());
        expected.extend_from_slice(&3u16.to_ne_bytes());
        assert_eq!(SocketAddrControl.to_bytes().to_vec(), expected);
    }

    #[test]
    fn an_l2cap_address_is_a_sockaddr_l2() {
        // PSM 1 (service discovery) to 11:22:33:44:55:66, BR/EDR: family, psm (le16), bdaddr in
        // wire order, cid (le16), bdaddr_type, pad.
        let address = SocketAddrL2 { psm: 1, address: "11:22:33:44:55:66".parse().unwrap(), cid: 0, kind: AddressType::BrEdr };
        let mut expected = Vec::new();
        expected.extend_from_slice(&31u16.to_ne_bytes());
        expected.extend_from_slice(&[0x01, 0x00, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0x00, 0x00, 0x00, 0x00]);
        assert_eq!(address.to_bytes().to_vec(), expected);
        // The attribute channel, CID 4, to an LE random address (type 2).
        let address = SocketAddrL2 { psm: 0, address: "11:22:33:44:55:66".parse().unwrap(), cid: 4, kind: AddressType::LeRandom };
        assert_eq!(address.to_bytes()[2..], [0x00, 0x00, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0x04, 0x00, 0x02, 0x00]);
    }
}
