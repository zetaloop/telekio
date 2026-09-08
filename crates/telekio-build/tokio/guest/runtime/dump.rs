use super::*;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

pub(crate) async fn dump<F>(handle: &crate::runtime::Handle, original: F) -> Dump
where
    F: std::future::Future<Output = Dump>,
{
    let _ = original;
    let ::telekio_abi::DumpResult { call, mut dump } = handle.inner.connection().handle.dump();
    call.resume("failed to start Tokio runtime dump");
    let bytes = std::future::poll_fn(|context| {
        let waker = unsafe { ::telekio_abi::Waker::from_ref(context.waker()) };
        dump.poll(&waker)
    })
    .await;
    decode(&bytes)
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ForeignFrame {
    pub(crate) ip: usize,
    pub(crate) symbol: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct ForeignTrace {
    pub(crate) backtraces: Vec<Vec<ForeignFrame>>,
}

#[derive(Clone)]
struct ForeignSymbol {
    symbol: BacktraceSymbol,
    parent_hash: u64,
}

impl Trace {
    /// Resolve and return a list of backtraces that are involved in polls in this trace.
    ///
    /// The exact backtraces included here are unstable and might change in the future,
    /// but you can expect one [`Backtrace`] for every call to
    /// [`poll`] to a bottom-level Tokio future - so if something like [`join!`] is
    /// used, there will be a backtrace for each future in the join.
    ///
    /// [`poll`]: std::future::Future::poll
    /// [`join!`]: macro@join
    pub fn resolve_backtraces(&self) -> Vec<Backtrace> {
        let Some(trace) = self.inner.telekio_foreign() else {
            return self.resolve_backtraces_local();
        };
        trace
            .backtraces
            .iter()
            .map(|backtrace| Backtrace {
                frames: backtrace.iter().map(resolve).collect(),
            })
            .collect()
    }
}

pub(crate) fn decode(bytes: &[u8]) -> Dump {
    let mut bytes = bytes;
    let tasks = (0..read(&mut bytes))
        .map(|_| {
            let id = crate::runtime::task::Id::from_telekio(read(&mut bytes));
            let backtraces = (0..read(&mut bytes))
                .map(|_| {
                    (0..read(&mut bytes))
                        .map(|_| ForeignFrame {
                            ip: read(&mut bytes)
                                .try_into()
                                .expect("Tokio dump address overflow"),
                            symbol: read(&mut bytes)
                                .try_into()
                                .expect("Tokio dump address overflow"),
                        })
                        .collect()
                })
                .collect();
            Task {
                id,
                trace: Trace {
                    inner: crate::runtime::task::trace::Trace::from_telekio(backtraces),
                },
            }
        })
        .collect();
    assert!(bytes.is_empty(), "Tokio runtime dump has trailing data");
    Dump::new(tasks)
}

pub(crate) fn display(trace: &ForeignTrace, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    let traces = trace
        .backtraces
        .iter()
        .map(|backtrace| {
            let mut trace = Vec::new();
            let mut state = DefaultHasher::new();
            for frame in backtrace.iter().rev() {
                for symbol in symbols(frame).into_iter().rev() {
                    let symbol = ForeignSymbol {
                        symbol,
                        parent_hash: state.finish(),
                    };
                    symbol.hash(&mut state);
                    trace.push(symbol);
                }
            }
            trace
        })
        .collect();
    crate::runtime::task::trace::telekio::display_foreign(traces, formatter)
}

fn resolve(frame: &ForeignFrame) -> BacktraceFrame {
    BacktraceFrame {
        ip: Address(frame.ip as *mut std::ffi::c_void),
        symbol_address: Address(frame.symbol as *mut std::ffi::c_void),
        symbols: symbols(frame).into_boxed_slice(),
    }
}

fn symbols(frame: &ForeignFrame) -> Vec<BacktraceSymbol> {
    let mut symbols = Vec::new();
    backtrace::resolve(frame.ip as *mut std::ffi::c_void, |symbol| {
        let name = symbol.name();
        symbols.push(BacktraceSymbol {
            name: name.as_ref().map(|name| name.as_bytes().into()),
            name_demangled: name.map(|name| format!("{name:#}").into()),
            addr: symbol.addr().map(Address),
            filename: symbol.filename().map(From::from),
            lineno: symbol.lineno(),
            colno: symbol.colno(),
        });
    });
    symbols
}

impl Hash for ForeignSymbol {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.symbol.name.hash(state);
        self.symbol.addr.map(|addr| addr.0).hash(state);
        self.symbol.filename.hash(state);
        self.symbol.lineno.hash(state);
        self.symbol.colno.hash(state);
        self.parent_hash.hash(state);
    }
}

impl PartialEq for ForeignSymbol {
    fn eq(&self, other: &Self) -> bool {
        self.parent_hash == other.parent_hash
            && self.symbol.name == other.symbol.name
            && self.symbol.addr.map(|addr| addr.0) == other.symbol.addr.map(|addr| addr.0)
            && self.symbol.filename == other.symbol.filename
            && self.symbol.lineno == other.symbol.lineno
            && self.symbol.colno == other.symbol.colno
    }
}

impl Eq for ForeignSymbol {}

impl fmt::Display for ForeignSymbol {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(name) = &self.symbol.name_demangled {
            let name = name
                .rsplit_once("::")
                .map_or(name.as_ref(), |(name, _)| name);
            formatter.write_str(name)?;
        }
        if let Some(filename) = &self.symbol.filename {
            write!(formatter, " at {}", filename.to_string_lossy())?;
            if let Some(line) = self.symbol.lineno {
                write!(formatter, ":{line}")?;
                if let Some(column) = self.symbol.colno {
                    write!(formatter, ":{column}")?;
                }
            }
        }
        Ok(())
    }
}

fn read(bytes: &mut &[u8]) -> u64 {
    let (value, rest) = bytes.split_at(8);
    *bytes = rest;
    u64::from_le_bytes(value.try_into().unwrap())
}
