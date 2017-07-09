use std::cmp;
use std::io::{self, Read, Write};
use std::path::Path;
use std::fs::File;

use futures::{Future, Stream};

use http::hyper;

/// The number of bytes to read into the "peek" buffer.
const PEEK_BYTES: usize = 512;

/// Type representing the data in the body of an incoming request.
///
/// This type is the only means by which the body of a request can be retrieved.
/// This type is not usually used directly. Instead, types that implement
/// [FromData](/rocket/data/trait.FromData.html) are used via code generation by
/// specifying the `data = "<param>"` route parameter as follows:
///
/// ```rust,ignore
/// #[post("/submit", data = "<var>")]
/// fn submit(var: T) -> ... { ... }
/// ```
///
/// Above, `T` can be any type that implements `FromData`. Note that `Data`
/// itself implements `FromData`.
///
/// # Reading Data
///
/// Data may be read from a `Data` object by calling either the
/// [open](#method.open) or [peek](#method.peek) methods.
///
/// The `open` method consumes the `Data` object and returns the raw data
/// stream. The `Data` object is consumed for safety reasons: consuming the
/// object ensures that holding a `Data` object means that all of the data is
/// available for reading.
///
/// The `peek` method returns a slice containing at most 512 bytes of buffered
/// body data. This enables partially or fully reading from a `Data` object
/// without consuming the `Data` object.
pub struct Data {
    buffer: Vec<u8>,
    stream: Option<hyper::Body>,
}

/// Raw data stream of a request body.
///
/// This stream can only be obtained by calling
/// [Data::open](/rocket/data/struct.Data.html#method.open). The stream contains
/// all of the data in the body of the request. It exposes no methods directly.
/// Instead, it must be used as an opaque `Read` structure.
// TODO: In the long term (full async) we want to get rid of this entirely!
pub struct DataStream(usize, Data);

// TODO: Have a `BufRead` impl for `DataStream`. At the moment, this isn't
// possible since Hyper's `HttpReader` doesn't implement `BufRead`.
impl Read for DataStream {
    #[inline(always)]
    fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
        trace_!("DataStream::read()");
        let buf = &mut self.1.buffer;

        if buf.len() <= self.0 {
            if let Some(stream) = self.1.stream.take() {
                buf.truncate(0);
                self.0 = 0;
                match stream.into_future().wait() {
                    Ok((chunk, stream)) => {
                        if let Some(chunk) = chunk {
                            buf.extend_from_slice(&chunk);
                        }
                        self.1.stream = Some(stream);
                    }
                    Err(_) => return Err(io::Error::new(io::ErrorKind::Other, "")),
                }
            }
        }
        let buf = &mut buf[self.0..];
        if buf.is_empty() {
            return Ok(0);
        }
        let amount = cmp::min(buf.len(), target.len());
        target[..amount].copy_from_slice(&buf[..amount]);
        self.0 += amount;
        Ok(amount)
    }
}

impl Data {
    /// Returns the raw data stream.
    ///
    /// The stream contains all of the data in the body of the request,
    /// including that in the `peek` buffer. The method consumes the `Data`
    /// instance. This ensures that a `Data` type _always_ represents _all_ of
    /// the data in a request.
    ///
    /// # Example
    ///
    /// ```rust
    /// use rocket::Data;
    ///
    /// fn handler(data: Data) {
    ///     let stream = data.open();
    /// }
    /// ```
    pub fn open(self) -> DataStream {
        DataStream(0, self)
    }

    pub(crate) fn from_hyp(body: hyper::Body) -> Data {
        Data {
            buffer: vec![],
            stream: Some(body),
        }
    }

    /// Retrieve the `peek` buffer.
    ///
    /// The peek buffer contains at most 512 bytes of the body of the request.
    /// The actual size of the returned buffer varies by web request. The
    /// [`peek_complete`](#method.peek_complete) can be used to determine if
    /// this buffer contains _all_ of the data in the body of the request.
    ///
    /// # Example
    ///
    /// ```rust
    /// use rocket::Data;
    ///
    /// fn handler(data: Data) {
    ///     let peek = data.peek();
    /// }
    /// ```
    #[inline(always)]
    pub fn peek(&self) -> &[u8] {
        if self.buffer.len() > PEEK_BYTES {
            &self.buffer[..PEEK_BYTES]
        } else {
            &self.buffer
        }
    }

    /// Returns true if the `peek` buffer contains all of the data in the body
    /// of the request. Returns `false` if it does not or if it is not known if
    /// it does.
    ///
    /// # Example
    ///
    /// ```rust
    /// use rocket::Data;
    ///
    /// fn handler(data: Data) {
    ///     if data.peek_complete() {
    ///         println!("All of the data: {:?}", data.peek());
    ///     }
    /// }
    /// ```
    #[inline(always)]
    pub fn peek_complete(&self) -> bool {
        self.stream.is_none()
    }

    /// A helper method to write the body of the request to any `Write` type.
    ///
    /// This method is identical to `io::copy(&mut data.open(), writer)`.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::io;
    /// use rocket::Data;
    ///
    /// fn handler(mut data: Data) -> io::Result<String> {
    ///     // write all of the data to stdout
    ///     data.stream_to(&mut io::stdout())
    ///         .map(|n| format!("Wrote {} bytes.", n))
    /// }
    /// ```
    #[inline(always)]
    pub fn stream_to<W: Write>(self, writer: &mut W) -> io::Result<u64> {
        io::copy(&mut self.open(), writer)
    }

    /// A helper method to write the body of the request to a file at the path
    /// determined by `path`.
    ///
    /// This method is identical to
    /// `io::copy(&mut self.open(), &mut File::create(path)?)`.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::io;
    /// use rocket::Data;
    ///
    /// fn handler(mut data: Data) -> io::Result<String> {
    ///     data.stream_to_file("/static/file")
    ///         .map(|n| format!("Wrote {} bytes to /static/file", n))
    /// }
    /// ```
    #[inline(always)]
    pub fn stream_to_file<P: AsRef<Path>>(self, path: P) -> io::Result<u64> {
        io::copy(&mut self.open(), &mut File::create(path)?)
    }

    /*TODO: ? // Creates a new data object with an internal buffer `buf`, where the cursor
    // in the buffer is at `pos` and the buffer has `cap` valid bytes. Thus, the
    // bytes `vec[pos..cap]` are buffered and unread. The remainder of the data
    // bytes can be read from `stream`.
    #[inline(always)]
    pub(crate) fn new(mut stream: BodyReader) -> Data {
        trace_!("Date::new({:?})", stream);
        let mut peek_buf = vec![0; PEEK_BYTES];

        // Fill the buffer with as many bytes as possible. If we read less than
        // that buffer's length, we know we reached the EOF. Otherwise, it's
        // unclear, so we just say we didn't reach EOF.
        let eof = match stream.read_max(&mut peek_buf[..]) {
            Ok(n) => {
                trace_!("Filled peek buf with {} bytes.", n);
                // TODO: Explain this.
                unsafe { peek_buf.set_len(n); }
                n < PEEK_BYTES
            }
            Err(e) => {
                error_!("Failed to read into peek buffer: {:?}.", e);
                unsafe { peek_buf.set_len(0); }
                false
            },
        };

        trace_!("Peek bytes: {}/{} bytes.", peek_buf.len(), PEEK_BYTES);
        Data {
            buffer: peek_buf,
            stream: stream,
            is_complete: eof,
        }
    }*/

    /// This creates a `data` object from a local data source `data`.
    #[inline]
    pub(crate) fn local(data: Vec<u8>) -> Data {
        Data {
            buffer: data,
            stream: None,
        }
    }
}
