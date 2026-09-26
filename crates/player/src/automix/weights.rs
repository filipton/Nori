//! Beat This!'s weights, made on the device from the authors' own checkpoint. The app carries the network without
//! its weights ([`GRAPH`], 0.2 MB, made by tools/beat-this/export.py): every initializer in it is external data, at
//! its place in one weights file, with the recipe that makes it from the checkpoint's state_dict. [`convert`] follows
//! those recipes once, when the checkpoint has been downloaded and checked, and the core keeps what it writes (the
//! pinned `beat_model::SHA256`); [`assemble`] puts that file's bytes back into the graph for tract.
//!
//! The recipes (keys of each initializer's `external_data`; the initializer's type says fp32 or fp16):
//! - `copy`: the state_dict tensor `nori.from` as it is;
//! - `transpose`: a Linear's weight, turned as the ONNX exporter folds it into a MatMul;
//! - `conv_bn`: a Conv2d's weight with the BatchNorm2d `nori.bn` after it folded in, as the exporter folds it:
//!   w * gamma / sqrt(var + eps) per output channel, `nori.eps` the BatchNorm's epsilon;
//! - `bn_shift`: the bias that fold leaves, beta - mean * gamma / sqrt(var + eps), of the BatchNorm `nori.from`.
//!
//! Each step is one float32 operation, rounded as IEEE 754 says, so every platform writes the same bytes, and
//! export.py's numpy copy of the recipes pins them. PyTorch's own fold used a square root that is not always
//! correctly rounded: 130 of the 2.1 M values differ from the export the app used to ship by one unit in the last
//! place, which moves no logit by more than a few millionths (the test below, with the checkpoint).

use std::collections::HashMap;

use prost::Message;
use tract_onnx::pb::{tensor_proto, ModelProto, TensorProto};
use tract_onnx::prelude::f16;

use super::checkpoint::{self, Tensor};

/// Beat This! small0 as ONNX, its weights left out.
pub static GRAPH: &[u8] = include_bytes!("../../models/beat-this-small0.graph.onnx");

const FLOAT: i32 = tensor_proto::DataType::Float as i32;
const FLOAT16: i32 = tensor_proto::DataType::Float16 as i32;
const EXTERNAL: i32 = tensor_proto::DataLocation::External as i32;

/// Where an initializer's bytes are in the weights file.
fn place(t: &TensorProto) -> Result<(usize, usize), String> {
    let get = |k: &str| t.external_data.iter().find(|e| e.key == k).map(|e| e.value.as_str());
    let num = |k: &str| get(k).and_then(|v| v.parse::<usize>().ok()).ok_or_else(|| format!("{}: no {k}", t.name));
    Ok((num("offset")?, num("length")?))
}

/// The graph's initializers that come from the weights file, in the order they lie in it.
fn external(model: &ModelProto) -> Result<Vec<&TensorProto>, String> {
    let graph = model.graph.as_ref().ok_or("no graph")?;
    Ok(graph.initializer.iter().filter(|t| t.data_location == Some(EXTERNAL)).collect())
}

fn graph() -> Result<ModelProto, String> {
    ModelProto::decode(GRAPH).map_err(|e| format!("the graph: {e}"))
}

/// The weights file the graph expects, made from the checkpoint's bytes (`small0.ckpt`, checked by the caller).
pub fn convert(ckpt: &[u8]) -> Result<Vec<u8>, String> {
    let sd = checkpoint::state_dict(ckpt)?;
    let model = graph()?;
    let mut out = Vec::new();
    for t in external(&model)? {
        let get = |k: &str| t.external_data.iter().find(|e| e.key == k).map(|e| e.value.as_str());
        let tensor = |k: &str| sd.get(k).ok_or_else(|| format!("the checkpoint has no {k}"));
        let eps = || get("nori.eps").and_then(|e| e.parse::<f64>().ok()).map(|e| e as f32).ok_or_else(|| format!("{}: no epsilon", t.name));
        let from = get("nori.from").ok_or_else(|| format!("{}: no source", t.name))?;
        let dims: Vec<usize> = t.dims.iter().map(|d| usize::try_from(*d).map_err(|_| format!("{}: a negative size", t.name))).collect::<Result<_, _>>()?;
        let values = match get("nori.op") {
            Some("copy") => tensor(from)?.clone(),
            Some("transpose") => transpose(tensor(from)?)?,
            Some("conv_bn") => {
                let (w, s) = (tensor(from)?, bn_scale(&sd, get("nori.bn").unwrap_or(""), eps()?)?);
                let per = w.numel() / s.len().max(1);
                if w.shape.first() != Some(&s.len()) {
                    return Err(format!("{from} and its BatchNorm disagree"));
                }
                Tensor { shape: w.shape.clone(), data: w.data.iter().enumerate().map(|(i, v)| v * s[i / per]).collect() }
            }
            Some("bn_shift") => {
                let s = bn_scale(&sd, from, eps()?)?;
                let (beta, mean) = (tensor(&format!("{from}.bias"))?, tensor(&format!("{from}.running_mean"))?);
                if beta.numel() != s.len() || mean.numel() != s.len() {
                    return Err(format!("{from}: its parts disagree"));
                }
                Tensor { shape: vec![s.len()], data: beta.data.iter().zip(&mean.data).zip(&s).map(|((b, m), s)| b - m * s).collect() }
            }
            op => return Err(format!("{}: no recipe {op:?}", t.name)),
        };
        if values.shape != dims {
            return Err(format!("{}: {from} is {:?}, the graph wants {dims:?}", t.name, values.shape));
        }
        let (offset, length) = place(t)?;
        if offset != out.len() {
            return Err(format!("{}: not where the last one ended", t.name));
        }
        match t.data_type {
            FLOAT => out.extend(values.data.iter().flat_map(|v| v.to_le_bytes())),
            FLOAT16 => out.extend(values.data.iter().flat_map(|v| f16::from_f32(*v).to_bits().to_le_bytes())),
            _ => return Err(format!("{}: neither fp32 nor fp16", t.name)),
        }
        if out.len() != offset + length {
            return Err(format!("{}: {} bytes, the graph says {length}", t.name, out.len() - offset));
        }
    }
    Ok(out)
}

/// A 2-D tensor turned.
fn transpose(t: &Tensor) -> Result<Tensor, String> {
    let [r, c] = t.shape[..] else { return Err("a transpose of something not 2-D".into()) };
    let data = (0..c).flat_map(|j| (0..r).map(move |i| (i, j))).map(|(i, j)| t.data[i * c + j]).collect();
    Ok(Tensor { shape: vec![c, r], data })
}

/// gamma / sqrt(var + eps) per channel of the BatchNorm `bn`, one float32 rounding per step.
fn bn_scale(sd: &HashMap<String, Tensor>, bn: &str, eps: f32) -> Result<Vec<f32>, String> {
    let get = |k: &str| sd.get(&format!("{bn}.{k}")).ok_or_else(|| format!("the checkpoint has no {bn}.{k}"));
    let (gamma, var) = (get("weight")?, get("running_var")?);
    if gamma.numel() != var.numel() {
        return Err(format!("{bn}: its parts disagree"));
    }
    Ok(gamma.data.iter().zip(&var.data).map(|(g, v)| g / (v + eps).sqrt()).collect())
}

/// The graph with the weights file's bytes in it, for tract.
pub fn assemble(weights: &[u8]) -> Result<ModelProto, String> {
    let mut model = graph()?;
    let graph = model.graph.as_mut().ok_or("no graph")?;
    let mut end = 0;
    for t in graph.initializer.iter_mut().filter(|t| t.data_location == Some(EXTERNAL)) {
        let (offset, length) = place(t)?;
        t.raw_data = weights.get(offset..offset + length).ok_or_else(|| format!("{}: past the end of the weights", t.name))?.to_vec();
        t.external_data.clear();
        t.data_location = None;
        end = end.max(offset + length);
    }
    if end != weights.len() {
        return Err(format!("{} bytes of weights, the graph reads {end}", weights.len()));
    }
    Ok(model)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automix::neural::{BeatThis, LogMel, CHUNK, MELS};

    /// The graph carries no weights: every initializer comes from the weights file, and what the file does not
    /// hold is too small to be one.
    #[test]
    fn the_graph_carries_no_weights() {
        let model = graph().unwrap();
        let g = model.graph.as_ref().unwrap();
        assert!(g.initializer.iter().all(|t| t.data_location == Some(EXTERNAL) && t.raw_data.is_empty()));
        let ext = external(&model).unwrap();
        let total: usize = ext.iter().map(|t| place(t).unwrap().1).sum();
        assert_eq!((ext.len(), total), (138, 4_229_216));
        let biggest = g.node.iter().flat_map(|n| &n.attribute).filter_map(|a| a.t.as_ref()).map(|t| t.raw_data.len()).max().unwrap_or(0);
        assert!(biggest < 64, "a constant of {biggest} bytes");
        assert!(GRAPH.len() < 256 << 10, "{} bytes", GRAPH.len());
        assert!(assemble(&[0u8; 10]).is_err(), "a short file is refused");
    }

    /// Equal outputs. The weights made from the authors' checkpoint (`NORI_BEAT_THIS_CKPT`, small0.ckpt) are
    /// the pinned bytes; with the export the app shipped before (`NORI_BEAT_THIS`, tools/beat-this/export.py
    /// --full, SHA-256 847b51aa...) each weight is compared with its own and both models are run over the same
    /// windows: a spectrogram-like input (export.py's kind) and a drum loop through the app's own front end.
    /// `NORI_BEAT_THIS_CKPT=small0.ckpt NORI_BEAT_THIS=beat-this-small0-v1.onnx cargo test --release -p nori-player
    /// --features neural-beats official_weights -- --nocapture`
    #[test]
    #[ignore = "needs the authors' checkpoint in NORI_BEAT_THIS_CKPT"]
    fn official_weights_give_the_shipped_outputs() {
        let Ok(ckpt) = std::env::var("NORI_BEAT_THIS_CKPT") else {
            eprintln!("no checkpoint in NORI_BEAT_THIS_CKPT: skipped");
            return;
        };
        let ckpt = std::fs::read(ckpt).unwrap();
        let rss = || {
            let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
            s.lines().find_map(|l| l.strip_prefix("VmHWM:").map(|v| v.trim().to_string())).unwrap_or_default()
        };
        let before = rss();
        let t0 = std::time::Instant::now();
        let weights = convert(&ckpt).unwrap();
        let converted = t0.elapsed();
        let pin = sha(&weights);
        println!("converted in {:.0} ms, {} bytes, SHA-256 {pin}; peak RSS {before} before, {} after", converted.as_secs_f64() * 1e3, weights.len(), rss());
        assert_eq!(pin, "e9349da04b9da4ad41c5e416c71a9471af3a416249e7addef0101b3d569df5a7", "export.py's numpy makes the same bytes");
        let t1 = std::time::Instant::now();
        let ours = BeatThis::from_weights(&weights).unwrap();
        println!("assembled and loaded in {:.0} ms; peak RSS {}", t1.elapsed().as_secs_f64() * 1e3, rss());

        let Ok(shipped) = std::env::var("NORI_BEAT_THIS") else { return };
        let shipped = std::fs::read(shipped).unwrap();
        assert_eq!(sha(&shipped), "847b51aaef519a60a47c815fa58440782de73bff7000210396673b0353e2cc8c", "the export the app shipped");
        // Weight by weight: the same graph, the same initializers, byte for byte but for the folded ones.
        let old = ModelProto::decode(&shipped[..]).unwrap();
        let new = assemble(&weights).unwrap();
        let (og, ng) = (old.graph.unwrap(), new.graph.unwrap());
        assert_eq!((og.node.len(), og.initializer.len()), (ng.node.len(), ng.initializer.len()));
        let (mut same, mut differ, mut worst) = (0, 0, 0f32);
        for (o, n) in og.initializer.iter().zip(&ng.initializer) {
            assert_eq!((o.dims.clone(), o.data_type), (n.dims.clone(), n.data_type));
            let vals = |t: &TensorProto| -> Vec<f32> {
                if t.data_type == FLOAT16 {
                    t.raw_data.chunks_exact(2).map(|b| f16::from_bits(u16::from_le_bytes([b[0], b[1]])).to_f32()).collect()
                } else {
                    t.raw_data.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
                }
            };
            for (a, b) in vals(o).iter().zip(vals(n)) {
                if *a == b {
                    same += 1;
                } else {
                    differ += 1;
                    worst = worst.max((a - b).abs() / a.abs().max(1e-6));
                }
            }
        }
        println!("weights: {same} the same, {differ} not (largest relative difference {worst:.1e})");
        assert!(differ <= 200 && worst < 1e-3);

        let before = BeatThis::from_bytes(&shipped).unwrap();
        let mut rng = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng >> 40) as f32 / (1u64 << 24) as f32
        };
        let noise: Vec<[f32; MELS]> = (0..CHUNK - 12).map(|_| std::array::from_fn(|_| next() * 6.0)).collect();
        // Four bars of a kick, a snare and hats at 124 BPM over a bass line, 30 s at 22.05 kHz.
        let rate = 22_050.0;
        let beat = 60.0 / 124.0;
        let song: Vec<f32> = (0..(30.0 * rate) as usize)
            .map(|i| {
                let t = i as f64 / rate;
                let (n, p) = ((t / beat).floor() as i64, t % beat);
                let kick = if n % 2 == 0 { (-p * 30.0).exp() * (2.0 * std::f64::consts::PI * 55.0 * p).sin() } else { 0.0 };
                let snare = if n % 2 == 1 { (-p * 25.0).exp() * (next() as f64 * 2.0 - 1.0) * 0.6 } else { 0.0 };
                let hat = (-((t / (beat / 2.0)) % 1.0) * beat / 2.0 * 80.0).exp() * (next() as f64 * 2.0 - 1.0) * 0.2;
                let bass = 0.2 * (2.0 * std::f64::consts::PI * [41.2, 41.2, 49.0, 36.7][(n / 4 % 4) as usize] * t).sin();
                (kick + snare + hat + bass) as f32 * 0.5
            })
            .collect();
        let drums = LogMel::new(rate).frames(&song);
        for (name, mel) in [("spectrogram-like input", &noise), ("drum loop", &drums)] {
            let (b0, d0) = before.logits(mel).unwrap();
            let (b1, d1) = ours.logits(mel).unwrap();
            let diff = b0.iter().zip(&b1).chain(d0.iter().zip(&d1)).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
            let flips = b0.iter().zip(&b1).chain(d0.iter().zip(&d1)).filter(|(a, b)| (**a > 0.0) != (**b > 0.0)).count();
            let (t0, t1) = (before.track(mel, true, true).unwrap(), ours.track(mel, true, true).unwrap());
            println!("{name}: {} frames, largest logit difference {diff:.2e}, frames on the other side of zero {flips}, {} beats and {} downbeats either way", b0.len(), t1.beats.len(), t1.downbeats.len());
            assert!(diff < 1e-3, "{name}: {diff}");
            assert_eq!(flips, 0, "{name}");
            assert_eq!((t0.beats, t0.downbeats), (t1.beats, t1.downbeats), "{name}: the same beats");
        }
    }

    fn sha(b: &[u8]) -> String {
        use sha2::Digest;
        sha2::Sha256::digest(b).iter().map(|b| format!("{b:02x}")).collect()
    }
}
