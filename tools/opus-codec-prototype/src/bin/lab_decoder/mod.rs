use sonimesh_audio::FrameDecoder;

pub struct Decoder {
    inner: opus::Decoder,
    reset_error: Option<opus::Error>,
}

impl Decoder {
    pub fn new() -> Result<Self, opus::Error> {
        Ok(Self {
            inner: opus::Decoder::new(48_000, opus::Channels::Stereo)?,
            reset_error: None,
        })
    }
}

impl FrameDecoder for Decoder {
    type Error = opus::Error;

    fn decode(&mut self, input: &[u8], output: &mut [f32]) -> Result<usize, Self::Error> {
        if let Some(error) = self.reset_error.take() {
            return Err(error);
        }
        self.inner.decode_float(input, output, false).map(|n| n * 2)
    }

    fn decode_plc(&mut self, output: &mut [f32]) -> Result<usize, Self::Error> {
        if let Some(error) = self.reset_error.take() {
            return Err(error);
        }
        self.inner.decode_float(&[], output, false).map(|n| n * 2)
    }

    fn reset(&mut self) {
        if self.reset_error.is_none() {
            self.reset_error = self.inner.reset_state().err();
        }
    }
}
