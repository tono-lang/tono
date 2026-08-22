//! A stand-in for a third-party numeric library with the shapes real Rust
//! libraries have, kept deliberately (this is the FFI bench, not a fixture
//! bent to fit the emitter): the logical handle is a trait, and every
//! constructor returns a *different* concrete struct implementing it; the
//! formula constructor takes an options struct by value (with a
//! `Default`); the series constructor takes a `Vec`; and the fallback
//! constructor takes a `Vec` of boxed trait objects, the handles the caller
//! already built. No handle is `Clone`.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

/// The library's own error: a formula that does not parse is its own
/// variant, so a caller recognises it by pattern.
#[derive(Debug)]
pub enum Error {
    Parse(String),
    Other(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parse(s) | Error::Other(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for Error {}

/// The future a [`Calculator::compute`] answers with: boxed so the trait
/// stays object safe (`from_fallback` takes `Vec<Box<dyn Calculator<T>>>`).
pub type Computed<'a, T> = Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'a>>;

/// The logical handle. Object safe on purpose.
pub trait Calculator<T>: Send + Sync {
    fn compute(&self) -> Computed<'_, T>;
}

pub struct ConstantCalculator<T> {
    value: T,
}

pub fn from_constant<T>(value: T) -> Result<ConstantCalculator<T>, Error> {
    Ok(ConstantCalculator { value })
}

impl<T: Clone + Send + Sync> Calculator<T> for ConstantCalculator<T> {
    fn compute(&self) -> Computed<'_, T> {
        Box::pin(async move { Ok(self.value.clone()) })
    }
}

/// Options for [`from_formula`], taken by value; a caller with nothing to
/// set passes `FormulaOptions::default()`.
#[derive(Default, Debug, Clone)]
pub struct FormulaOptions {
    pub precision: Option<u8>,
}

pub struct FormulaCalculator<T> {
    expr: String,
    precision: Option<u8>,
    _marker: std::marker::PhantomData<T>,
}

pub fn from_formula<T>(expr: &str, options: FormulaOptions) -> Result<FormulaCalculator<T>, Error> {
    Ok(FormulaCalculator {
        expr: expr.to_string(),
        precision: options.precision,
        _marker: std::marker::PhantomData,
    })
}

/// Evaluates "<a> <op> <b>". Only the `f64` instantiation computes (the
/// one the bench exercises); other `T` fail loudly at compute time.
impl<T: Send + Sync + 'static> Calculator<T> for FormulaCalculator<T> {
    fn compute(&self) -> Computed<'_, T> {
        Box::pin(async move { self.evaluate() })
    }
}

impl<T> FormulaCalculator<T> {
    /// An associated function on the type itself (`FormulaCalculator::parse`),
    /// the shape many crates use for parsing, next to the free
    /// [`from_formula`]: reached through the type, not a free function.
    pub fn parse(expr: impl AsRef<str>) -> Result<Self, Error> {
        Ok(FormulaCalculator {
            expr: expr.as_ref().to_string(),
            precision: None,
            _marker: std::marker::PhantomData,
        })
    }
}

impl<T: 'static> FormulaCalculator<T> {
    fn evaluate(&self) -> Result<T, Error> {
        let fields: Vec<&str> = self.expr.split_whitespace().collect();
        if fields.len() != 3 {
            return Err(Error::Parse(format!("mathkit: cannot parse {:?}", self.expr)));
        }
        let parse = |s: &str| s.parse::<f64>().map_err(|e| Error::Parse(e.to_string()));
        let a = parse(fields[0])?;
        let b = parse(fields[2])?;
        let mut out = match fields[1] {
            "+" => a + b,
            "-" => a - b,
            "*" => a * b,
            "/" => a / b,
            op => return Err(Error::Other(format!("mathkit: unknown operator {op:?}"))),
        };
        if let Some(digits) = self.precision {
            let scale = 10f64.powi(i32::from(digits));
            out = (out * scale).round() / scale;
        }
        let boxed: Box<dyn std::any::Any> = Box::new(out);
        boxed
            .downcast::<T>()
            .map(|v| *v)
            .map_err(|_| Error::Other("mathkit: formula results are f64".to_string()))
    }
}

pub struct SeriesCalculator<T> {
    values: Vec<T>,
}

pub fn from_series<T>(values: Vec<T>) -> Result<SeriesCalculator<T>, Error> {
    Ok(SeriesCalculator { values })
}

/// Answers the last value of the series.
impl<T: Clone + Send + Sync> Calculator<T> for SeriesCalculator<T> {
    fn compute(&self) -> Computed<'_, T> {
        Box::pin(async move {
            self.values
                .last()
                .cloned()
                .ok_or_else(|| Error::Other("mathkit: empty series".to_string()))
        })
    }
}

pub struct FallbackCalculator<T> {
    strategy: String,
    calcs: Vec<Box<dyn Calculator<T>>>,
}

/// Composes calculators the caller already built: the library takes its own
/// handles back, boxed behind the trait, in a collection. `strategy` is
/// owned (not `&str`): the collection argument is the shape capability 5
/// exercises, and a scalar argument's own borrowed-vs-owned shape is a
/// separate, unrelated capability the emitter does not distinguish yet
/// (every call-argument position renders a scalar string as owned).
pub fn from_fallback<T>(
    strategy: String,
    calcs: Vec<Box<dyn Calculator<T>>>,
) -> Result<FallbackCalculator<T>, Error> {
    if strategy != "first" && strategy != "last" {
        return Err(Error::Other("mathkit: unknown fallback strategy".to_string()));
    }
    Ok(FallbackCalculator { strategy, calcs })
}

/// Asks each composed calculator in turn ("first" answers the first
/// success, "last" the last one).
impl<T: Send + Sync> Calculator<T> for FallbackCalculator<T> {
    fn compute(&self) -> Computed<'_, T> {
        Box::pin(async move {
            let mut last: Result<T, Error> =
                Err(Error::Other("mathkit: no calculators to fall back to".to_string()));
            for c in &self.calcs {
                match c.compute().await {
                    Ok(v) if self.strategy == "first" => return Ok(v),
                    other => last = other,
                }
            }
            last
        })
    }
}
