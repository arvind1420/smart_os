use super::syscall::*;

pub struct TcpStream {
    fd: u64,
}

impl TcpStream {
    pub fn connect(host: &str, port: u16) -> Option<Self> {
        let ip = syscall3(SYS_GETHOSTBYNAME, host.as_ptr() as u64, host.len() as u64, 0);
        if ip == u64::MAX { return None; }
        
        let fd = syscall3(SYS_TCP_CONNECT, ip, port as u64, 0);
        if fd == u64::MAX { return None; }
        
        Some(Self { fd })
    }

    pub fn send(&self, data: &[u8]) -> bool {
        syscall3(SYS_TCP_SEND, self.fd, data.as_ptr() as u64, data.len() as u64) != u64::MAX
    }

    pub fn recv(&self, buf: &mut [u8]) -> Option<usize> {
        let ret = syscall3(SYS_TCP_RECV, self.fd, buf.as_mut_ptr() as u64, buf.len() as u64);
        if ret == u64::MAX { None } else { Some(ret as usize) }
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        syscall1(SYS_TCP_CLOSE, self.fd);
    }
}
