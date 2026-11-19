use std::io;
use std::sync::mpsc;

pub(crate) type Input = Box<dyn FnMut() -> Result<(), mpsc::TryRecvError>>;

#[cfg(not(target_os = "wasi"))]
fn read_input(mut reader: impl io::Read, tx: mpsc::Sender<()>) {
    let mut bytes = [0; 1024];
    loop {
        match reader.read(&mut bytes) {
            Ok(0) | Err(_) => break,
            Ok(_) => if tx.send(()).is_err() { break; },
        }
    }
}

pub(crate) fn input() -> Result<Input, String> {
    #[cfg(not(target_os = "wasi"))]
    {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || read_input(io::stdin(), tx));
        Ok(Box::new(move || rx.try_recv()))
    }
    #[cfg(target_os = "wasi")]
    {
        use std::io::Read;
        #[link(wasm_import_module = "wasi_snapshot_preview1")]
        unsafe extern "C" {
            fn fd_fdstat_set_flags(fd: u32, flags: u32) -> u32;
        }
        if unsafe { fd_fdstat_set_flags(0, 4) } != 0 {
            return Err("holder: nonblocking stdin unavailable\n".into());
        }
        Ok(Box::new(move || {
            let mut bytes = [0; 1024];
            match io::stdin().read(&mut bytes) {
                Ok(0) => Err(mpsc::TryRecvError::Disconnected),
                Ok(_) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => Err(mpsc::TryRecvError::Empty),
                Err(_) => Err(mpsc::TryRecvError::Disconnected),
            }
        }))
    }
}
