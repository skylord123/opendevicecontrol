//! A Bluetooth Classic RFCOMM stream socket, and the list of paired
//! Bluetooth devices that might be Bose headphones.
//!
//! Linux uses a BlueZ `AF_BLUETOOTH` socket and asks BlueZ over D-Bus which
//! devices are connected. Windows uses a Winsock `AF_BTH` socket and the
//! Bluetooth API's device enumeration.
//!
//! BMAP runs on the headphones' standard Serial Port Profile service
//! (UUID 0x1101, "SPP Dev"), which is what the Bose app connects to
//! (`com.bose.bmap.service.SppConnectionManager`). The vendor UUID
//! `00000000-deca-fade-…` the headphones also list is a different service
//! (Apple iAP2: it answers `ff 55 …`), not BMAP. Windows finds the SPP
//! channel itself from the UUID; on Linux [`sdp_channel`] asks the device's
//! SDP server, since BlueZ has no client-side SDP lookup over D-Bus.

use crate::{Error, Result};
use std::time::Duration;

/// The Serial Port Profile service class BMAP runs on.
pub const SPP_UUID: &str = "00001101-0000-1000-8000-00805f9b34fb";
/// The same, as bytes (big-endian).
pub const SPP: [u8; 16] = [0x00, 0x00, 0x11, 0x01, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb];
/// The "SRfcomm" service (`9b26d8c0-a8ed-440b-95b0-c4714a518bcc`): the
/// headphones send a message on it each time the shortcut button is held
/// while set to Spotify Tap (how Spotify Tap reaches the Spotify app).
pub const TAP: [u8; 16] = [0x9b, 0x26, 0xd8, 0xc0, 0xa8, 0xed, 0x44, 0x0b, 0x95, 0xb0, 0xc4, 0x71, 0x4a, 0x51, 0x8b, 0xcc];
/// Bose's Bluetooth SIG company id, as BlueZ shows it in a device's
/// `Modalias` (`bluetooth:v009Ep4075d0106`).
pub const BOSE_MODALIAS: &str = "bluetooth:v009E";

/// A paired, connected Bluetooth device that looks like a Bose product.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub address: [u8; 6],
    pub name: String,
    /// The Bluetooth Device ID product id (from the modalias), when known.
    pub product: Option<u16>,
}

pub fn format_address(a: &[u8; 6]) -> String {
    a.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

pub fn parse_address(s: &str) -> Option<[u8; 6]> {
    let mut out = [0u8; 6];
    let mut parts = s.split(':');
    for b in &mut out {
        *b = u8::from_str_radix(parts.next()?, 16).ok()?;
    }
    parts.next().is_none().then_some(out)
}

/// The few SDP (Service Discovery Protocol) PDUs needed to find the BMAP
/// service's RFCOMM channel. Pure byte handling, so it is the same on every
/// platform and testable without a radio.
mod sdp {
    /// ServiceSearchAttributeRequest for the ProtocolDescriptorList
    /// (attribute 0x0004) of the service `uuid` (128-bit; SDP matches a
    /// short UUID by its full form too). `continuation` is the state from
    /// the last answer (`[0]` the first time).
    pub fn request(tid: u16, uuid: &[u8; 16], continuation: &[u8]) -> Vec<u8> {
        let mut params = vec![0x35, 0x11, 0x1c];
        params.extend_from_slice(uuid);
        params.extend_from_slice(&[0x02, 0x00]); // max attribute bytes per answer
        params.extend_from_slice(&[0x35, 0x03, 0x09, 0x00, 0x04]);
        params.extend_from_slice(continuation);
        let mut out = vec![0x06];
        out.extend_from_slice(&tid.to_be_bytes());
        out.extend_from_slice(&(params.len() as u16).to_be_bytes());
        out.extend(params);
        out
    }

    /// A ServiceSearchAttributeResponse: (attribute bytes, continuation state).
    pub fn response(pdu: &[u8]) -> Option<(&[u8], &[u8])> {
        if pdu.len() < 7 || pdu[0] != 0x07 {
            return None;
        }
        let count = u16::from_be_bytes([pdu[5], pdu[6]]) as usize;
        let attrs = pdu.get(7..7 + count)?;
        let cont = pdu.get(7 + count..)?;
        Some((attrs, cont))
    }

    /// One data element: (type, payload, total length).
    fn element(b: &[u8]) -> Option<(u8, &[u8], usize)> {
        let d = *b.first()?;
        let (kind, size) = (d >> 3, d & 7);
        let (hdr, len) = match (kind, size) {
            (0, _) => (1, 0),
            (_, 0..=4) => (1, 1usize << size),
            (_, 5) => (2, *b.get(1)? as usize),
            (_, 6) => (3, u16::from_be_bytes([*b.get(1)?, *b.get(2)?]) as usize),
            (_, _) => (5, u32::from_be_bytes([*b.get(1)?, *b.get(2)?, *b.get(3)?, *b.get(4)?]) as usize),
        };
        Some((kind, b.get(hdr..hdr + len)?, hdr + len))
    }

    /// The RFCOMM channel in a list of attribute lists: the unsigned
    /// integer that follows UUID 0x0003 (RFCOMM) in a protocol descriptor.
    pub fn rfcomm_channel(b: &[u8]) -> Option<u8> {
        let mut i = 0;
        while i < b.len() {
            let (kind, data, len) = element(&b[i..])?;
            match kind {
                // Sequence or alternative: look inside.
                6 | 7 => {
                    if let Some(c) = rfcomm_channel(data) {
                        return Some(c);
                    }
                }
                // UUID 0x0003, then the channel as uint8.
                3 if data == [0x00, 0x03] => {
                    let (k, d, _) = element(&b[i + len..])?;
                    if k == 1 && d.len() == 1 {
                        return Some(d[0]);
                    }
                }
                _ => {}
            }
            i += len;
        }
        None
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn finds_the_channel() {
            // One record: attribute 0x0004 = ((L2CAP), (RFCOMM, 8)).
            let attrs = [
                0x35, 0x14, 0x35, 0x12, 0x09, 0x00, 0x04, 0x35, 0x0d, 0x35, 0x03, 0x19, 0x01, 0x00, 0x35, 0x05, 0x19, 0x00, 0x03, 0x08, 0x08,
                0x00,
            ];
            assert_eq!(rfcomm_channel(&attrs), Some(8));
            let mut pdu = vec![0x07, 0x00, 0x01, 0x00, 0x00, 0x00, attrs.len() as u8];
            pdu.extend_from_slice(&attrs);
            pdu.push(0);
            let (a, c) = response(&pdu).unwrap();
            assert_eq!(a, attrs);
            assert_eq!(c, [0]);
        }

        #[test]
        fn reads_a_real_answer() {
            // Shaped like the Bose QC Headphones' answer (their SPP record
            // is channel 8). The outer sequences use the 16-bit length form
            // (0x36).
            let pdu = [
                0x07, 0x00, 0x01, 0x00, 0x1a, 0x00, 0x17, 0x36, 0x00, 0x14, 0x36, 0x00, 0x11, 0x09, 0x00, 0x04, 0x35, 0x0c, 0x35, 0x03, 0x19, 0x01,
                0x00, 0x35, 0x05, 0x19, 0x00, 0x03, 0x08, 0x08, 0x00,
            ];
            let (attrs, cont) = response(&pdu).unwrap();
            assert_eq!(cont, [0]);
            assert_eq!(rfcomm_channel(attrs), Some(8));
        }

        #[test]
        fn request_layout() {
            let r = request(1, &super::super::SPP, &[0]);
            assert_eq!(&r[..5], &[0x06, 0x00, 0x01, 0x00, (r.len() - 5) as u8]);
            assert_eq!(&r[5..12], &[0x35, 0x11, 0x1c, 0x00, 0x00, 0x11, 0x01]);
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    const AF_BLUETOOTH: libc::c_int = 31;
    const BTPROTO_L2CAP: libc::c_int = 0;
    const BTPROTO_RFCOMM: libc::c_int = 3;
    /// The SDP server's L2CAP channel.
    const SDP_PSM: u16 = 1;

    #[repr(C)]
    struct SockaddrRc {
        family: libc::sa_family_t,
        /// Little-endian (reversed) address, as BlueZ stores it.
        bdaddr: [u8; 6],
        channel: u8,
    }

    #[repr(C)]
    struct SockaddrL2 {
        family: libc::sa_family_t,
        psm: u16,
        bdaddr: [u8; 6],
        cid: u16,
        bdaddr_type: u8,
    }

    pub struct Socket(OwnedFd);

    fn os_err(what: &str) -> Error {
        Error::Other(format!("Bluetooth {what}: {}", std::io::Error::last_os_error()))
    }

    fn reversed(address: &[u8; 6]) -> [u8; 6] {
        let mut a = *address;
        a.reverse();
        a
    }

    impl Socket {
        /// Connect to the RFCOMM channel of service `uuid`, looked up over SDP.
        pub fn connect_service(address: &[u8; 6], uuid: &[u8; 16], timeout: Duration) -> Result<Socket> {
            let channel = sdp_channel(address, uuid, timeout)?;
            Socket::connect(address, channel, timeout)
        }

        pub fn connect(address: &[u8; 6], channel: u8, timeout: Duration) -> Result<Socket> {
            let addr = SockaddrRc { family: AF_BLUETOOTH as libc::sa_family_t, bdaddr: reversed(address), channel };
            Socket::open(libc::SOCK_STREAM, BTPROTO_RFCOMM, &addr, timeout)
        }

        /// A connected socket of `kind`/`proto` to `addr` (a sockaddr_*).
        fn open<A>(kind: libc::c_int, proto: libc::c_int, addr: &A, timeout: Duration) -> Result<Socket> {
            // SAFETY: plain socket(2); the descriptor is owned right away.
            let fd = unsafe { libc::socket(AF_BLUETOOTH, kind | libc::SOCK_CLOEXEC, proto) };
            if fd < 0 {
                let e = std::io::Error::last_os_error();
                let hint = if e.raw_os_error() == Some(libc::EAFNOSUPPORT) { " (no Bluetooth support in this kernel)" } else { "" };
                return Err(Error::Other(format!("Bluetooth socket: {e}{hint}")));
            }
            // SAFETY: fd is a fresh, valid descriptor nobody else owns.
            let sock = Socket(unsafe { OwnedFd::from_raw_fd(fd) });
            // Connect without blocking forever on a device that is out of
            // reach: non-blocking connect, then poll for writability.
            sock.set_nonblocking(true)?;
            // SAFETY: addr is a properly initialised sockaddr of its size.
            let r = unsafe { libc::connect(fd, addr as *const A as *const libc::sockaddr, std::mem::size_of::<A>() as libc::socklen_t) };
            if r < 0 {
                let e = std::io::Error::last_os_error();
                if e.raw_os_error() != Some(libc::EINPROGRESS) {
                    return Err(Error::Other(format!("Bluetooth connect: {e}")));
                }
                if !sock.wait(libc::POLLOUT, timeout)? {
                    return Err(Error::Other("Bluetooth connect: timed out".into()));
                }
                let mut err: libc::c_int = 0;
                let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
                // SAFETY: reading SO_ERROR into a c_int.
                unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_ERROR, &mut err as *mut _ as *mut libc::c_void, &mut len) };
                if err != 0 {
                    return Err(Error::Other(format!("Bluetooth connect: {}", std::io::Error::from_raw_os_error(err))));
                }
            }
            Ok(sock)
        }

        fn set_nonblocking(&self, on: bool) -> Result<()> {
            let fd = self.0.as_raw_fd();
            // SAFETY: fcntl on our own descriptor.
            unsafe {
                let flags = libc::fcntl(fd, libc::F_GETFL);
                let flags = if on { flags | libc::O_NONBLOCK } else { flags & !libc::O_NONBLOCK };
                if libc::fcntl(fd, libc::F_SETFL, flags) < 0 {
                    return Err(os_err("fcntl"));
                }
            }
            Ok(())
        }

        /// Wait until `events` is ready or `timeout` passes.
        fn wait(&self, events: libc::c_short, timeout: Duration) -> Result<bool> {
            let mut p = libc::pollfd { fd: self.0.as_raw_fd(), events, revents: 0 };
            let ms = timeout.as_millis().min(i32::MAX as u128) as libc::c_int;
            // SAFETY: one valid pollfd.
            let r = unsafe { libc::poll(&mut p, 1, ms) };
            if r < 0 {
                return Err(os_err("poll"));
            }
            Ok(r > 0)
        }

        pub fn send(&self, data: &[u8]) -> Result<()> {
            let mut sent = 0;
            while sent < data.len() {
                if !self.wait(libc::POLLOUT, Duration::from_secs(2))? {
                    return Err(Error::Other("Bluetooth send: timed out".into()));
                }
                // SAFETY: the slice is valid for its length.
                let n = unsafe { libc::send(self.0.as_raw_fd(), data[sent..].as_ptr() as *const libc::c_void, data.len() - sent, libc::MSG_NOSIGNAL) };
                if n < 0 {
                    let e = std::io::Error::last_os_error();
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        continue;
                    }
                    return Err(Error::Other(format!("Bluetooth send: {e}")));
                }
                sent += n as usize;
            }
            Ok(())
        }

        /// Read what is available, waiting at most `timeout`. Ok(0 bytes)
        /// means nothing arrived; a closed link is an error.
        pub fn recv(&self, buf: &mut [u8], timeout: Duration) -> Result<usize> {
            if !self.wait(libc::POLLIN, timeout)? {
                return Ok(0);
            }
            // SAFETY: buf is valid for its length.
            let n = unsafe { libc::recv(self.0.as_raw_fd(), buf.as_mut_ptr() as *mut libc::c_void, buf.len(), 0) };
            if n < 0 {
                let e = std::io::Error::last_os_error();
                if e.kind() == std::io::ErrorKind::WouldBlock {
                    return Ok(0);
                }
                return Err(Error::Other(format!("Bluetooth receive: {e}")));
            }
            if n == 0 {
                return Err(Error::Offline);
            }
            Ok(n as usize)
        }
    }

    /// The RFCOMM channel of the BMAP service, asked from the device's SDP
    /// server directly over L2CAP. This avoids libbluetooth and `sdptool`
    /// (deprecated and missing on many distributions), so it works the same
    /// on every distribution with a Bluetooth-enabled kernel.
    pub fn sdp_channel(address: &[u8; 6], uuid: &[u8; 16], timeout: Duration) -> Result<u8> {
        let addr = SockaddrL2 { family: AF_BLUETOOTH as libc::sa_family_t, psm: SDP_PSM.to_le(), bdaddr: reversed(address), cid: 0, bdaddr_type: 0 };
        let sock = Socket::open(libc::SOCK_SEQPACKET, BTPROTO_L2CAP, &addr, timeout)?;
        let mut attrs = Vec::new();
        let mut continuation = vec![0u8];
        for tid in 1u16..=16 {
            sock.send(&super::sdp::request(tid, uuid, &continuation))?;
            let mut buf = [0u8; 1024];
            let n = sock.recv(&mut buf, timeout)?;
            if n == 0 {
                return Err(Error::Other("SDP: no answer".into()));
            }
            let (chunk, cont) = super::sdp::response(&buf[..n]).ok_or_else(|| Error::Other("SDP: bad answer".into()))?;
            attrs.extend_from_slice(chunk);
            if cont.first().copied().unwrap_or(0) == 0 {
                break;
            }
            continuation = cont.to_vec();
        }
        super::sdp::rfcomm_channel(&attrs).ok_or_else(|| Error::Other("SDP: the device doesn't have that service".into()))
    }

    /// Connected BlueZ devices made by Bose that have a serial port service.
    pub fn candidates() -> Vec<Candidate> {
        use std::collections::HashMap;
        use zbus::zvariant::{OwnedObjectPath, OwnedValue};
        type Props = HashMap<String, OwnedValue>;
        type Objects = HashMap<OwnedObjectPath, HashMap<String, Props>>;
        let Ok(conn) = zbus::blocking::Connection::system() else { return Vec::new() };
        let Ok(proxy) = zbus::blocking::Proxy::new(&conn, "org.bluez", "/", "org.freedesktop.DBus.ObjectManager") else {
            return Vec::new();
        };
        let objects: Objects = match proxy.call("GetManagedObjects", &()) {
            Ok(o) => o,
            Err(e) => {
                // Some distributions limit BlueZ's D-Bus API to a group.
                static WARNED: std::sync::Once = std::sync::Once::new();
                WARNED.call_once(|| {
                    log::info!("can't list Bluetooth devices from BlueZ ({e}); if Bluetooth is on, add your user to the `bluetooth` group")
                });
                return Vec::new();
            }
        };
        let mut out = Vec::new();
        for ifaces in objects.values() {
            let Some(dev) = ifaces.get("org.bluez.Device1") else { continue };
            let connected = dev.get("Connected").and_then(|v| bool::try_from(v).ok()).unwrap_or(false);
            let uuids: Vec<String> = dev.get("UUIDs").and_then(|v| v.try_clone().ok()).and_then(|v| Vec::<String>::try_from(v).ok()).unwrap_or_default();
            let modalias = dev.get("Modalias").and_then(|v| String::try_from(v.try_clone().ok()?).ok()).unwrap_or_default();
            let bose = modalias.get(..BOSE_MODALIAS.len()).is_some_and(|m| m.eq_ignore_ascii_case(BOSE_MODALIAS));
            if !connected || !bose || !uuids.iter().any(|u| u.eq_ignore_ascii_case(SPP_UUID)) {
                continue;
            }
            let product = modalias.get(BOSE_MODALIAS.len()..).and_then(|r| r.strip_prefix('p')).and_then(|r| r.get(..4)).and_then(|h| u16::from_str_radix(h, 16).ok());
            let Some(address) = dev.get("Address").and_then(|v| String::try_from(v.try_clone().ok()?).ok()).and_then(|s| parse_address(&s)) else {
                continue;
            };
            let name = dev
                .get("Alias")
                .or_else(|| dev.get("Name"))
                .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
                .unwrap_or_default();
            out.push(Candidate { address, name, product });
        }
        out.sort_by_key(|c| c.address);
        out
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows_sys::core::GUID;
    use windows_sys::Win32::Devices::Bluetooth::*;
    use windows_sys::Win32::Networking::WinSock::*;

    pub struct Socket(SOCKET);

    // SAFETY: a SOCKET is a plain handle, used from one thread at a time.
    unsafe impl Send for Socket {}

    impl Drop for Socket {
        fn drop(&mut self) {
            // SAFETY: closing our own socket.
            unsafe { closesocket(self.0) };
        }
    }

    fn wsa_err(what: &str) -> Error {
        // SAFETY: no preconditions.
        let code = unsafe { WSAGetLastError() };
        Error::Other(format!("Bluetooth {what}: {}", std::io::Error::from_raw_os_error(code)))
    }

    fn startup() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            // SAFETY: WSADATA is plain data filled by WSAStartup.
            let mut data: WSADATA = unsafe { std::mem::zeroed() };
            unsafe { WSAStartup(0x0202, &mut data) };
        });
    }

    fn guid(u: &[u8; 16]) -> GUID {
        GUID {
            data1: u32::from_be_bytes([u[0], u[1], u[2], u[3]]),
            data2: u16::from_be_bytes([u[4], u[5]]),
            data3: u16::from_be_bytes([u[6], u[7]]),
            data4: [u[8], u[9], u[10], u[11], u[12], u[13], u[14], u[15]],
        }
    }

    fn bth_addr(a: &[u8; 6]) -> u64 {
        a.iter().fold(0u64, |acc, b| (acc << 8) | *b as u64)
    }

    impl Socket {
        /// Connect to service `uuid`; Windows looks its channel up (SDP).
        pub fn connect_service(address: &[u8; 6], uuid: &[u8; 16], timeout: Duration) -> Result<Socket> {
            Socket::open(address, 0, Some(guid(uuid)), timeout)
        }

        pub fn connect(address: &[u8; 6], channel: u8, timeout: Duration) -> Result<Socket> {
            Socket::open(address, channel, None, timeout)
        }

        fn open(address: &[u8; 6], channel: u8, service: Option<GUID>, timeout: Duration) -> Result<Socket> {
            startup();
            // SAFETY: plain socket(); owned right away.
            let s = unsafe { socket(AF_BTH as i32, SOCK_STREAM, BTHPROTO_RFCOMM as i32) };
            if s == INVALID_SOCKET {
                return Err(wsa_err("socket"));
            }
            let sock = Socket(s);
            let addr = SOCKADDR_BTH {
                addressFamily: AF_BTH,
                btAddr: bth_addr(address),
                serviceClassId: service.unwrap_or(unsafe { std::mem::zeroed() }),
                port: channel as u32,
            };
            let mut nb: u32 = 1;
            // SAFETY: FIONBIO on our socket.
            unsafe { ioctlsocket(s, FIONBIO, &mut nb) };
            // SAFETY: addr is a valid SOCKADDR_BTH.
            let r = unsafe { connect(s, &addr as *const SOCKADDR_BTH as *const SOCKADDR, std::mem::size_of::<SOCKADDR_BTH>() as i32) };
            if r == SOCKET_ERROR {
                // SAFETY: no preconditions.
                if unsafe { WSAGetLastError() } != WSAEWOULDBLOCK {
                    return Err(wsa_err("connect"));
                }
                let mut p = WSAPOLLFD { fd: s, events: POLLWRNORM as i16, revents: 0 };
                // SAFETY: one valid pollfd.
                let n = unsafe { WSAPoll(&mut p, 1, timeout.as_millis().min(i32::MAX as u128) as i32) };
                if n <= 0 {
                    return Err(Error::Other("Bluetooth connect: timed out".into()));
                }
                if p.revents & (POLLERR | POLLHUP) as i16 != 0 {
                    let mut err: i32 = 0;
                    let mut len = std::mem::size_of::<i32>() as i32;
                    // SAFETY: reading SO_ERROR into an i32.
                    unsafe { getsockopt(s, SOL_SOCKET, SO_ERROR, &mut err as *mut i32 as *mut u8, &mut len) };
                    return Err(Error::Other(format!("Bluetooth connect: {}", std::io::Error::from_raw_os_error(err))));
                }
            }
            Ok(sock)
        }

        fn wait(&self, events: i16, timeout: Duration) -> Result<bool> {
            let mut p = WSAPOLLFD { fd: self.0, events, revents: 0 };
            // SAFETY: one valid pollfd.
            let n = unsafe { WSAPoll(&mut p, 1, timeout.as_millis().min(i32::MAX as u128) as i32) };
            if n == SOCKET_ERROR {
                return Err(wsa_err("poll"));
            }
            Ok(n > 0)
        }

        pub fn send(&self, data: &[u8]) -> Result<()> {
            let mut sent = 0;
            while sent < data.len() {
                if !self.wait(POLLWRNORM as i16, Duration::from_secs(2))? {
                    return Err(Error::Other("Bluetooth send: timed out".into()));
                }
                let chunk = &data[sent..];
                // SAFETY: the slice is valid for its length.
                let n = unsafe { send(self.0, chunk.as_ptr(), chunk.len() as i32, 0) };
                if n == SOCKET_ERROR {
                    // SAFETY: no preconditions.
                    if unsafe { WSAGetLastError() } == WSAEWOULDBLOCK {
                        continue;
                    }
                    return Err(wsa_err("send"));
                }
                sent += n as usize;
            }
            Ok(())
        }

        pub fn recv(&self, buf: &mut [u8], timeout: Duration) -> Result<usize> {
            if !self.wait(POLLRDNORM as i16, timeout)? {
                return Ok(0);
            }
            // SAFETY: buf is valid for its length.
            let n = unsafe { recv(self.0, buf.as_mut_ptr(), buf.len() as i32, 0) };
            if n == SOCKET_ERROR {
                // SAFETY: no preconditions.
                if unsafe { WSAGetLastError() } == WSAEWOULDBLOCK {
                    return Ok(0);
                }
                return Err(wsa_err("receive"));
            }
            if n == 0 {
                return Err(Error::Offline);
            }
            Ok(n as usize)
        }
    }

    /// Connected Bluetooth audio devices named like a Bose product. Windows
    /// has no cheap way to read a device's maker, so the name has to do;
    /// the caller still checks that the device speaks BMAP.
    pub fn candidates() -> Vec<Candidate> {
        let mut out = Vec::new();
        let params = BLUETOOTH_DEVICE_SEARCH_PARAMS {
            dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32,
            fReturnAuthenticated: 0,
            fReturnRemembered: 0,
            fReturnUnknown: 0,
            fReturnConnected: 1,
            fIssueInquiry: 0,
            cTimeoutMultiplier: 0,
            hRadio: std::ptr::null_mut(),
        };
        // SAFETY: plain data, dwSize set before use.
        let mut info: BLUETOOTH_DEVICE_INFO = unsafe { std::mem::zeroed() };
        info.dwSize = std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32;
        // SAFETY: valid params and info.
        let find = unsafe { BluetoothFindFirstDevice(&params, &mut info) };
        if find.is_null() {
            return out;
        }
        loop {
            // Major device class 4: audio/video.
            let major = (info.ulClassofDevice >> 8) & 0x1f;
            let len = info.szName.iter().position(|&c| c == 0).unwrap_or(info.szName.len());
            let name = String::from_utf16_lossy(&info.szName[..len]);
            if info.fConnected != 0 && major == 4 && name.to_ascii_lowercase().contains("bose") {
                // SAFETY: the union holds the address as a u64.
                let raw = unsafe { info.Address.Anonymous.ullLong };
                let mut address = [0u8; 6];
                for (i, b) in address.iter_mut().enumerate() {
                    *b = (raw >> (8 * (5 - i))) as u8;
                }
                out.push(Candidate { address, name, product: None });
            }
            // SAFETY: find is a valid search handle.
            if unsafe { BluetoothFindNextDevice(find, &mut info) } == 0 {
                break;
            }
        }
        // SAFETY: closing our search handle.
        unsafe { BluetoothFindDeviceClose(find) };
        out.sort_by_key(|c| c.address);
        out
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod imp {
    use super::*;
    pub struct Socket;
    impl Socket {
        pub fn connect(_: &[u8; 6], _: u8, _: Duration) -> Result<Socket> {
            Err(Error::NotSupported(" on this platform"))
        }
        pub fn connect_service(_: &[u8; 6], _: &[u8; 16], _: Duration) -> Result<Socket> {
            Err(Error::NotSupported(" on this platform"))
        }
        pub fn send(&self, _: &[u8]) -> Result<()> {
            Err(Error::NotSupported(""))
        }
        pub fn recv(&self, _: &mut [u8], _: Duration) -> Result<usize> {
            Err(Error::NotSupported(""))
        }
    }
    pub fn candidates() -> Vec<Candidate> {
        Vec::new()
    }
}

pub use imp::{candidates, Socket};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_round_trip() {
        let a = parse_address("0A:1B:2C:3D:4E:5F").unwrap();
        assert_eq!(a, [0x0a, 0x1b, 0x2c, 0x3d, 0x4e, 0x5f]);
        assert_eq!(format_address(&a), "0A:1B:2C:3D:4E:5F");
        assert!(parse_address("0A:1B:2C").is_none());
    }
}
