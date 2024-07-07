/// Uses the current nix library for bulk IO. If there's a way to use sendmmsg and
/// recvmmsg without a vector allocation, I haven't found it yet.
pub mod nix_current {
    use nix::sys::socket::{recvmmsg, sendmmsg, MsgFlags, MultiHeaders, SockaddrStorage};
    use std::io::{IoSlice, IoSliceMut};

    pub fn send(
        fd: i32,
        hdrs: &mut MultiHeaders<SockaddrStorage>,
        bufs: &[([u8; 32], Vec<u8>)],
        addrs: &[Option<SockaddrStorage>],
    ) -> anyhow::Result<usize> {
        let slices: Vec<_> = bufs
            .iter()
            .map(|(buf1, buf2)| [IoSlice::new(buf1), IoSlice::new(buf2)])
            .collect();
        Ok(sendmmsg(fd, hdrs, &slices, addrs, [], MsgFlags::empty())?.count())
    }

    pub fn recv(
        fd: i32,
        hdrs: &mut MultiHeaders<SockaddrStorage>,
        bufs: &mut [([u8; 32], Vec<u8>)],
    ) -> anyhow::Result<usize> {
        let mut slices: Vec<_> = bufs
            .iter_mut()
            .map(|(buf1, buf2)| [IoSliceMut::new(buf1), IoSliceMut::new(buf2)])
            .collect();
        Ok(recvmmsg(fd, hdrs, &mut slices, MsgFlags::empty(), None)?.count())
    }
}

/// Uses the nix library with my proposed changes. With those changes, we can now
/// pass an iterator to sendmmsg and recvmmsg to avoid any allocations.
pub mod nix_proposed {
    use nix_prop::sys::socket::{
        recvmmsg, sendmmsg, MsgFlags, MultiHeaders, MultiResults, SockaddrStorage,
    };
    use std::io::{IoSlice, IoSliceMut};

    pub fn send(
        fd: i32,
        hdrs: &mut MultiHeaders<SockaddrStorage>,
        bufs: &[([u8; 32], Vec<u8>)],
        addrs: &[Option<SockaddrStorage>],
    ) -> anyhow::Result<usize> {
        let slices = bufs
            .iter()
            .map(|(buf1, buf2)| [IoSlice::new(buf1), IoSlice::new(buf2)]);
        Ok(sendmmsg(fd, hdrs, slices, addrs, [], MsgFlags::empty())?.count())
    }

    pub fn recv<'a>(
        fd: i32,
        hdrs: &'a mut MultiHeaders<SockaddrStorage>,
        bufs: &'a mut [([u8; 32], Vec<u8>)],
    ) -> anyhow::Result<MultiResults<'a, SockaddrStorage>> {
        let slices = bufs
            .iter_mut()
            .map(|(buf1, buf2)| [IoSliceMut::new(buf1), IoSliceMut::new(buf2)]);
        Ok(recvmmsg(fd, hdrs, slices, MsgFlags::empty(), None)?)
    }
}

/// Both variants run the same test: create an ipv6 UDP sender and receiver socket.
/// For some number of times, send some number of messages to the receiver. Bulk
/// IO should work as expected.
/// The two implementations are almost identical, which is the point. Notice however
/// that the current variant has to collect a vector every time it sends or receives
/// while the proposed variant can reuse its collections between calls.
#[cfg(test)]
mod tests {
    use rand::{thread_rng, RngCore};
    use std::{
        net::{SocketAddr, UdpSocket},
        os::fd::AsRawFd,
        str::FromStr,
        thread::sleep,
        time::Duration,
    };

    const MSGS_PER_BATCH: usize = 10;

    #[test]
    fn proposed_io() -> anyhow::Result<()> {
        use crate::nix_proposed;
        use nix_prop::sys::socket::{MultiHeaders, SockaddrStorage};

        let os_assigned_addr = SocketAddr::from_str("[::1]:0").unwrap();

        let sender = UdpSocket::bind(os_assigned_addr)?;
        let receiver = UdpSocket::bind(os_assigned_addr)?;

        let mut rng = thread_rng();

        let send_addrs: Box<[_]> =
            vec![Some(SockaddrStorage::from(receiver.local_addr()?)); MSGS_PER_BATCH].into();

        let mut bufs = (0..MSGS_PER_BATCH)
            .map(|_| ([0u8; 32], vec![0u8; 256]))
            .collect::<Vec<_>>();

        let mut send_hdrs: MultiHeaders<SockaddrStorage> =
            MultiHeaders::preallocate(MSGS_PER_BATCH, None);

        let mut recv_hdrs: MultiHeaders<SockaddrStorage> =
            MultiHeaders::preallocate(MSGS_PER_BATCH, None);

        for _tick in 0..10 {
            for (hdr, payload) in &mut bufs {
                rng.fill_bytes(hdr);
                rng.fill_bytes(payload);
            }

            // Send many
            nix_proposed::send(sender.as_raw_fd(), &mut send_hdrs, &bufs, &send_addrs)?;
            println!("sent {} messages", bufs.len());

            sleep(Duration::from_millis(500));

            // Receive many
            let received =
                nix_proposed::recv(receiver.as_raw_fd(), &mut recv_hdrs, &mut bufs)?.count();
            assert_eq!(received, MSGS_PER_BATCH);
            println!("received {received} messages");
        }

        Ok(())
    }

    #[test]
    fn current_io() -> anyhow::Result<()> {
        use crate::nix_current;
        use nix::sys::socket::{MultiHeaders, SockaddrStorage};

        let os_assigned_addr = SocketAddr::from_str("[::1]:0").unwrap();

        let sender = UdpSocket::bind(os_assigned_addr)?;
        let receiver = UdpSocket::bind(os_assigned_addr)?;

        let mut rng = thread_rng();

        let send_addrs: Box<[_]> =
            vec![Some(SockaddrStorage::from(receiver.local_addr()?)); MSGS_PER_BATCH].into();

        let mut bufs = (0..MSGS_PER_BATCH)
            .map(|_| ([0u8; 32], vec![0u8; 256]))
            .collect::<Vec<_>>();

        let mut send_hdrs: MultiHeaders<SockaddrStorage> =
            MultiHeaders::preallocate(MSGS_PER_BATCH, None);

        let mut recv_hdrs: MultiHeaders<SockaddrStorage> =
            MultiHeaders::preallocate(MSGS_PER_BATCH, None);

        for _tick in 0..10 {
            for (hdr, payload) in &mut bufs {
                rng.fill_bytes(hdr);
                rng.fill_bytes(payload);
            }

            // Send many
            nix_current::send(sender.as_raw_fd(), &mut send_hdrs, &bufs, &send_addrs)?;
            println!("sent {} messages", bufs.len());

            sleep(Duration::from_millis(500));

            // Receive many
            let received = nix_current::recv(receiver.as_raw_fd(), &mut recv_hdrs, &mut bufs)?;
            assert_eq!(received, MSGS_PER_BATCH);
            println!("received {received} messages");
        }

        Ok(())
    }
}
