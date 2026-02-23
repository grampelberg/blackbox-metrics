use crate::BlackboxRecorder;

#[derive(Debug)]
pub struct DumpGuard {
    recorder: BlackboxRecorder,
}

impl DumpGuard {
    pub(crate) const fn new(recorder: BlackboxRecorder) -> Self {
        Self { recorder }
    }
}

impl Drop for DumpGuard {
    fn drop(&mut self) {
        eprintln!("{}", self.recorder.snapshot());
    }
}
