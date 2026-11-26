use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    ArithNotImplemented,
    BadComponentCount,
    BadComponentId(i32),
    BadDctCoefficient,
    BadDctSize(i32),
    BadHuffmanTable,
    BadJColorSpace,
    BadLength,
    BadMcuSize,
    BadPrecision(i32),
    BadProgression,
    BadRestart,
    BadSampling,
    BadState,
    BadVirtualAccess,
    ConversionNotImplemented,
    DhtIndex(i32),
    DqtIndex(i32),
    EmptyImage,
    EoiExpected,
    FractionalSamplingNotImplemented,
    ImageTooBig,
    NoHuffmanTable(i32),
    NoImage,
    NoQuantTable(i32),
    NoSoi,
    NotImplemented,
    SofDuplicate,
    SofNoSos,
    SofUnsupported(i32),
    SoiDuplicate,
    SosNoSof,
    TooLittleData,
    UnknownMarker(i32),
    WidthOverflow,
    Aborted,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Warning {
    AdobeTransform,
    BogusProgression,
    ExtraneousData,
    HitMarker,
    HuffmanBadCode,
    JfifMajor,
    MustResync,
    NotSequential,
    TooMuchData,
}

impl Warning {
    pub fn is_corrupt_data(self) -> bool {
        matches!(self, Warning::ExtraneousData | Warning::HitMarker | Warning::HuffmanBadCode | Warning::MustResync)
    }
}
