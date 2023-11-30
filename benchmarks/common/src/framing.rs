use std::{
    io::{self, BufRead as _, BufReader, BufWriter, Read, Write},
    mem,
};

pub struct StdBufFraming<T: Read + Write> {
    inner: BufReader<ReadableBufWriter<T>>,
    second_buf: Vec<u8>,
}

impl<T: Read + Write> StdBufFraming<T> {
    pub fn new(capacity: usize, inner: T) -> Self {
        Self {
            inner: BufReader::with_capacity(
                capacity,
                ReadableBufWriter {
                    inner: BufWriter::with_capacity(capacity, inner),
                },
            ),
            second_buf: vec![0; capacity],
        }
    }

    #[inline]
    pub fn flush(&mut self) -> io::Result<()> {
        self.inner.get_mut().inner.flush()
    }

    pub fn write_message(&mut self, message: &[u8]) -> io::Result<()> {
        let writer = &mut self.inner.get_mut().inner;
        writer.write_all(&message.len().to_le_bytes())?;
        writer.write_all(message)?;
        writer.flush()
    }
}

impl<T: Read + Write> StdBufFraming<T> {
    pub fn read_message<R>(&mut self, f: impl FnOnce(&[u8]) -> R) -> io::Result<R> {
        let reader = &mut self.inner;

        let size = {
            let mut size_buf = [0; mem::size_of::<usize>()];
            reader.read_exact(&mut size_buf)?;
            usize::from_le_bytes(size_buf)
        };

        // Good case: message is fully in buffer, use it.
        let buf = reader.fill_buf()?;
        if buf.len() >= size {
            let res = Ok(f(&buf[..size]));
            reader.consume(size);
            return res;
        }

        // Bad case: fill our own buffer.
        reader.read_exact(&mut self.second_buf[..size])?;
        Ok(f(&self.second_buf[..size]))
    }
}

struct ReadableBufWriter<T: Write> {
    inner: BufWriter<T>,
}

impl<T: Read + Write> Read for ReadableBufWriter<T> {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.get_mut().read(buf)
    }

    #[inline]
    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        self.inner.get_mut().read_vectored(bufs)
    }

    #[inline]
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        self.inner.get_mut().read_to_end(buf)
    }

    #[inline]
    fn read_to_string(&mut self, buf: &mut String) -> io::Result<usize> {
        self.inner.get_mut().read_to_string(buf)
    }

    #[inline]
    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        self.inner.get_mut().read_exact(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_message() -> io::Result<()> {
        let mut buf = vec![];
        buf.extend_from_slice(&3_usize.to_le_bytes());
        buf.extend_from_slice(b"abc");
        buf.extend_from_slice(&6_usize.to_le_bytes());
        buf.extend_from_slice(b"qwerty");
        buf.extend_from_slice(&12_usize.to_le_bytes());
        buf.extend_from_slice(b"qwertyuiop[]");
        let mut stream = StdBufFraming::new(16, io::Cursor::new(buf));
        stream.read_message(|m| assert_eq!(m, b"abc"))?;
        stream.read_message(|m| assert_eq!(m, b"qwerty"))?;
        stream.read_message(|m| assert_eq!(m, b"qwertyuiop[]"))?;
        Ok(())
    }
}
