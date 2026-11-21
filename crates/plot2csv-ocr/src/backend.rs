//! Inference backend abstraction: the browser uses onnxruntime-web through a
//! small JS bridge, native builds can use the `ort` crate.

use std::future::Future;
use std::pin::Pin;

#[derive(Clone, Debug)]
pub enum TensorData {
    F32(Vec<f32>),
    I64(Vec<i64>),
}

#[derive(Clone, Debug)]
pub struct Tensor {
    pub dims: Vec<usize>,
    pub data: TensorData,
}

impl Tensor {
    pub fn f32(dims: Vec<usize>, data: Vec<f32>) -> Self {
        debug_assert_eq!(dims.iter().product::<usize>(), data.len());
        Self { dims, data: TensorData::F32(data) }
    }
    pub fn i64(dims: Vec<usize>, data: Vec<i64>) -> Self {
        debug_assert_eq!(dims.iter().product::<usize>(), data.len());
        Self { dims, data: TensorData::I64(data) }
    }
    pub fn as_f32(&self) -> anyhow::Result<&[f32]> {
        match &self.data {
            TensorData::F32(v) => Ok(v),
            _ => anyhow::bail!("expected f32 tensor"),
        }
    }
    pub fn as_i64(&self) -> anyhow::Result<&[i64]> {
        match &self.data {
            TensorData::I64(v) => Ok(v),
            _ => anyhow::bail!("expected i64 tensor"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Model {
    Det,
    Rec,
}

impl Model {
    pub fn name(self) -> &'static str {
        match self {
            Model::Det => "det",
            Model::Rec => "rec",
        }
    }
}

pub type LocalFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// Runs one model. `outputs` lists the output names to fetch, in order.
pub trait OcrBackend {
    fn run<'a>(
        &'a self,
        model: Model,
        inputs: Vec<(&'static str, Tensor)>,
        outputs: &'a [&'static str],
    ) -> LocalFuture<'a, anyhow::Result<Vec<Tensor>>>;
}

#[cfg(feature = "native-ocr")]
pub mod native {
    use super::*;
    use ort::session::Session;
    use ort::value::Tensor as OrtTensor;
    use std::sync::Mutex;

    pub struct NativeOrt {
        _env: ort::environment::Environment,
        det: Mutex<Session>,
        rec: Mutex<Session>,
    }

    impl NativeOrt {
        pub fn new(det: &[u8], rec: &[u8]) -> anyhow::Result<Self> {
            let env = ort::init().with_telemetry(false).build().map_err(|e| anyhow::anyhow!("{e}"))?;
            let mk = |b: &[u8]| -> anyhow::Result<Session> {
                Ok(Session::builder(&env)
                    .map_err(|e| anyhow::anyhow!("{e}"))?
                    .with_intra_threads(4)
                    .map_err(|e| anyhow::anyhow!("{e}"))?
                    .commit_from_memory(b)
                    .map_err(|e| anyhow::anyhow!("{e}"))?)
            };
            let (det, rec) = (Mutex::new(mk(det)?), Mutex::new(mk(rec)?));
            Ok(Self { _env: env, det, rec })
        }

        fn run_sync(&self, model: Model, inputs: Vec<(&'static str, Tensor)>, outputs: &[&'static str]) -> anyhow::Result<Vec<Tensor>> {
            let mut sess = match model {
                Model::Det => self.det.lock().unwrap(),
                Model::Rec => self.rec.lock().unwrap(),
            };
            let mut feed: Vec<(std::borrow::Cow<'static, str>, ort::session::SessionInputValue<'static>)> = Vec::new();
            for (name, t) in inputs {
                let shape: Vec<i64> = t.dims.iter().map(|&d| d as i64).collect();
                let v: ort::session::SessionInputValue<'static> = match t.data {
                    TensorData::F32(d) => OrtTensor::from_array((shape, d)).map_err(|e| anyhow::anyhow!("{e}"))?.into(),
                    TensorData::I64(d) => OrtTensor::from_array((shape, d)).map_err(|e| anyhow::anyhow!("{e}"))?.into(),
                };
                feed.push((name.into(), v));
            }
            let res = sess.run(feed).map_err(|e| anyhow::anyhow!("{e}"))?;
            let mut out = Vec::new();
            for name in outputs {
                let v = &res[*name];
                if let Ok((shape, data)) = v.try_extract_tensor::<f32>() {
                    out.push(Tensor::f32(shape.iter().map(|&d| d as usize).collect(), data.to_vec()));
                } else {
                    let (shape, data) = v.try_extract_tensor::<i64>().map_err(|e| anyhow::anyhow!("{e}"))?;
                    out.push(Tensor::i64(shape.iter().map(|&d| d as usize).collect(), data.to_vec()));
                }
            }
            Ok(out)
        }
    }

    impl OcrBackend for NativeOrt {
        fn run<'a>(
            &'a self,
            model: Model,
            inputs: Vec<(&'static str, Tensor)>,
            outputs: &'a [&'static str],
        ) -> LocalFuture<'a, anyhow::Result<Vec<Tensor>>> {
            let r = self.run_sync(model, inputs, outputs);
            Box::pin(async move { r })
        }
    }
}
