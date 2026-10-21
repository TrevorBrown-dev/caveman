//! The named-pipe server the trainer talks to: one client at a time, one JSON line in, one JSON line out. All waits
//! are overlapped with short timeouts, so the server notices a stop request within a tenth of a second.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::win::{self, Handle};

/// Something that answers request lines; `stop` is set when the answer means the server must end (unload).
pub trait Answer {
    fn answer(&self, line: &str) -> (String, bool);
}

const BUF: u32 = 64 * 1024;
const POLL_MS: u32 = 100;
/// A client that sends nothing for this long is dropped.
const IDLE_MS: u64 = 60_000;
/// The longest request line accepted.
const MAX_LINE: usize = 4 << 20;

struct Overlapped {
    ov: win::OVERLAPPED,
    event: Handle,
}

impl Overlapped {
    fn new() -> Option<Overlapped> {
        let event = Handle(unsafe { win::CreateEventW(std::ptr::null_mut(), 1, 0, std::ptr::null()) });
        if !event.valid() {
            return None;
        }
        let mut ov: win::OVERLAPPED = unsafe { std::mem::zeroed() };
        ov.hEvent = event.0;
        Some(Overlapped { ov, event })
    }

    fn reset(&mut self) {
        unsafe { win::ResetEvent(self.event.0) };
        let ev = self.ov.hEvent;
        self.ov = unsafe { std::mem::zeroed() };
        self.ov.hEvent = ev;
    }
}

/// Waits for an overlapped operation on `pipe`; None when stopped or timed out (the operation is cancelled).
fn finish(pipe: win::HANDLE, o: &mut Overlapped, stop: &AtomicBool, timeout_ms: u64) -> Option<u32> {
    let start = win::now_ms();
    loop {
        match unsafe { win::WaitForSingleObject(o.event.0, POLL_MS) } {
            win::WAIT_OBJECT_0 => {
                let mut n = 0u32;
                let ok = unsafe { win::GetOverlappedResult(pipe, &mut o.ov, &mut n, 0) };
                return (ok != 0 || unsafe { win::GetLastError() } == win::ERROR_MORE_DATA).then_some(n);
            }
            win::WAIT_TIMEOUT if !stop.load(Ordering::Acquire) && win::now_ms() - start < timeout_ms => continue,
            _ => {
                unsafe { win::CancelIo(pipe) };
                let mut n = 0u32;
                unsafe { win::GetOverlappedResult(pipe, &mut o.ov, &mut n, 1) };
                return None;
            }
        }
    }
}

/// Serves `name` until `stop` is set. Err when the pipe cannot be created (another DLL instance owns it).
pub fn serve(name: &str, handler: &dyn Answer, stop: &AtomicBool) -> Result<(), String> {
    let wname = win::wide(name);
    let pipe = Handle(unsafe {
        win::CreateNamedPipeW(
            wname.as_ptr(),
            win::PIPE_ACCESS_DUPLEX | win::FILE_FLAG_OVERLAPPED | win::FILE_FLAG_FIRST_PIPE_INSTANCE,
            win::PIPE_TYPE_BYTE | win::PIPE_READMODE_BYTE | win::PIPE_WAIT | win::PIPE_REJECT_REMOTE_CLIENTS,
            1,
            BUF,
            BUF,
            0,
            std::ptr::null_mut(),
        )
    });
    if !pipe.valid() {
        return Err(format!("cannot create {name} (error {}): another caveman_fx.dll is loaded?", unsafe { win::GetLastError() }));
    }
    let mut o = Overlapped::new().ok_or("no event for the pipe")?;
    while !stop.load(Ordering::Acquire) {
        o.reset();
        let connected = if unsafe { win::ConnectNamedPipe(pipe.0, &mut o.ov) } != 0 {
            true
        } else {
            match unsafe { win::GetLastError() } {
                win::ERROR_PIPE_CONNECTED => true,
                win::ERROR_IO_PENDING => finish(pipe.0, &mut o, stop, u64::MAX).is_some(),
                _ => {
                    unsafe { win::Sleep(POLL_MS) };
                    false
                }
            }
        };
        if connected {
            client(pipe.0, &mut o, handler, stop);
        }
        unsafe {
            win::FlushFileBuffers(pipe.0);
            win::DisconnectNamedPipe(pipe.0);
        }
    }
    Ok(())
}

/// One client: lines in, answers out, until it hangs up, idles too long or the server stops.
fn client(pipe: win::HANDLE, o: &mut Overlapped, handler: &dyn Answer, stop: &AtomicBool) {
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; BUF as usize];
    loop {
        while let Some(nl) = pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = pending.drain(..=nl).collect();
            let text = String::from_utf8_lossy(&line).trim().to_string();
            if text.is_empty() {
                continue;
            }
            let (mut answer, ends) = handler.answer(&text);
            answer.push('\n');
            if !write_all(pipe, o, answer.as_bytes(), stop) {
                return;
            }
            if ends {
                unsafe { win::FlushFileBuffers(pipe) };
                stop.store(true, Ordering::Release);
                return;
            }
        }
        if pending.len() > MAX_LINE {
            return;
        }
        o.reset();
        let mut n = 0u32;
        let ok = unsafe { win::ReadFile(pipe, buf.as_mut_ptr().cast(), BUF, &mut n, &mut o.ov) };
        let got = if ok != 0 {
            Some(n)
        } else {
            match unsafe { win::GetLastError() } {
                win::ERROR_IO_PENDING => finish(pipe, o, stop, IDLE_MS),
                win::ERROR_MORE_DATA => Some(n),
                _ => None, // broken pipe: the client left
            }
        };
        match got {
            Some(0) | None => return,
            Some(n) => pending.extend_from_slice(&buf[..n as usize]),
        }
    }
}

fn write_all(pipe: win::HANDLE, o: &mut Overlapped, mut data: &[u8], stop: &AtomicBool) -> bool {
    while !data.is_empty() {
        o.reset();
        let mut n = 0u32;
        let chunk = data.len().min(BUF as usize) as u32;
        let ok = unsafe { win::WriteFile(pipe, data.as_ptr().cast(), chunk, &mut n, &mut o.ov) };
        let wrote = if ok != 0 {
            Some(n)
        } else if unsafe { win::GetLastError() } == win::ERROR_IO_PENDING {
            finish(pipe, o, stop, 5_000)
        } else {
            None
        };
        match wrote {
            Some(n) if n > 0 => data = &data[n as usize..],
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto;

    struct Echo;

    impl Answer for Echo {
        fn answer(&self, line: &str) -> (String, bool) {
            let v: serde_json::Value = serde_json::from_str(line).unwrap_or_default();
            let bye = v.get("cmd").and_then(|c| c.as_str()) == Some("unload");
            (serde_json::json!({"ok": true, "echo": v}).to_string(), bye)
        }
    }

    #[test]
    fn serves_and_stops() {
        static STOP: AtomicBool = AtomicBool::new(false);
        let name = format!(r"\\.\pipe\caveman-fx-unit-{}", std::process::id());
        let server = {
            let name = name.clone();
            std::thread::spawn(move || serve(&name, &Echo, &STOP))
        };
        let a = proto::request(&name, &proto::Request::Ping, 5000).unwrap();
        assert_eq!(a["echo"]["cmd"], "ping");
        // a second client after the first left
        let b = proto::request_line(&name, r#"{"cmd":"status","x":[1,2,3]}"#, 5000).unwrap();
        assert_eq!(b["echo"]["x"][2], 3);
        // a big line
        let big = format!(r#"{{"cmd":"status","pad":"{}"}}"#, "x".repeat(200_000));
        let c = proto::request_line(&name, &big, 5000).unwrap();
        assert_eq!(c["echo"]["pad"].as_str().unwrap().len(), 200_000);
        // a second server on the same name is refused
        static OTHER: AtomicBool = AtomicBool::new(true);
        assert!(serve(&name, &Echo, &OTHER).is_err());
        let d = proto::request(&name, &proto::Request::Unload, 5000).unwrap();
        assert_eq!(d["ok"], true);
        server.join().unwrap().unwrap();
        assert!(STOP.load(Ordering::Acquire));
        assert!(proto::request(&name, &proto::Request::Ping, 300).is_err());
    }
}
