#!/usr/bin/env python3
"""Makes the beat model Nori downloads for "Better beat detection" from the MIT sources, and checks it.

Beat This! (Foscarin, Schlüter and Widmer, ISMIR 2024; https://github.com/CPJKU/beat_this) publishes its code and
weights under the MIT licence. This takes the `small0` checkpoint (2.1 M parameters, 8.1 MB) and exports it to ONNX
with the repository's own model code, for one 30 s window (1512 frames of 128 mel bands: 1500 and a border of 6 on
each side) with both outputs as logits. Then two changes for the phone, each checked:

- Each attention the exporter spells out (q and k scaled, MatMul, Softmax, MatMul with v) becomes one ONNX
  `Attention` node, which tract runs as flash attention: the spelled-out form holds every 1512 x 1512 score matrix
  of 32 frequency rows at once, 700 MB at the peak, the fused one about 100 MB. `Attention` belongs to opset 23 while
  the rest of the file is opset 17, so onnxruntime will not open the result; tract, which reads operators by name,
  does, and the app only uses tract.
- The weights are stored as fp16 with a Cast back to fp32 in front of each: half the size, and tract still computes
  in fp32 (logits move by about 0.01; no frame changes side of zero on the check below). Dynamic int8 does not load
  in tract, and BitChord's int8 small export misbehaves; see docs/research/analysis.md.

    python3.13 -m venv /tmp/bt && /tmp/bt/bin/pip install torch==2.8.0 --index-url https://download.pytorch.org/whl/cpu
    /tmp/bt/bin/pip install onnx==1.19.0 onnxruntime numpy einops==0.8.0 rotary-embedding-torch==0.6.4
    git clone https://github.com/CPJKU/beat_this /tmp/beat_this && git -C /tmp/beat_this checkout b95c8ab0c58c
    /tmp/bt/bin/python tools/beat-this/export.py --beat-this /tmp/beat_this --out beat-this-small0-v1.onnx

rotary-embedding-torch is the version beat_this's requirements.txt names: 0.9.1 traces a different graph (a file
1.2 MB bigger). torch 2.8.0 has no wheel for Python 3.14. The checkpoint is fetched from the authors' server and
checked against its SHA-256 (or given with --checkpoint).
The fp16 export (before the attention is fused) is compared with the PyTorch model on a fixed test input. It prints
the file's size and SHA-256: the numbers that go in crates/automix/src/beat_model.rs with the file published as a
release asset. The fused file is then checked where it runs, in tract:

    NORI_BEAT_THIS=beat-this-small0-v1.onnx cargo test --release -p nori-engine --features neural-beats --test core
    NORI_BEAT_THIS=beat-this-small0-v1.onnx NORI_BEAT_THIS_REF=<the full model, final0, as ONNX> \
        cargo test --release -p nori-player --features neural-beats neural_eval -- --ignored --nocapture
"""
import argparse
import hashlib
import sys
import tempfile
import urllib.request
from pathlib import Path

import numpy as np

CHECKPOINT_URL = "https://cloud.cp.jku.at/public.php/dav/files/7ik4RrBKTS273gp/small0.ckpt"
CHECKPOINT_SHA256 = "6074be2c4d490c5f6101fcc374a1ec72ae93456e23bb6019783b849f5dc7d47b"
FRAMES, MELS = 1512, 128


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load_model(repo: Path, checkpoint: Path):
    import inspect

    import torch

    sys.path.insert(0, str(repo))
    from beat_this.model.beat_tracker import BeatThis
    from beat_this.utils import replace_state_dict_key

    ckpt = torch.load(checkpoint, map_location="cpu", weights_only=True)
    # As beat_this.inference.load_model does, without its audio dependencies: the small model's own sizes.
    hparams = {k: v for k, v in ckpt["hyper_parameters"].items() if k in set(inspect.signature(BeatThis).parameters)}
    model = BeatThis(**hparams)
    model.load_state_dict(replace_state_dict_key(ckpt["state_dict"], "model.", ""))
    return model.eval(), hparams


def export(model, path: Path):
    import torch

    class Logits(torch.nn.Module):
        def __init__(self, m):
            super().__init__()
            self.m = m

        def forward(self, spect):
            out = self.m(spect)
            return out["beat"], out["downbeat"]

    with torch.no_grad():
        # The frame count stays open, so the harness can also run the file over other chunk lengths.
        frames = {"spect": {1: "frames"}, "beat": {1: "frames"}, "downbeat": {1: "frames"}}
        torch.onnx.export(
            Logits(model), torch.zeros(1, FRAMES, MELS), str(path), input_names=["spect"],
            output_names=["beat", "downbeat"], dynamic_axes=frames, opset_version=17, dynamo=False,
        )


def to_fp16_weights(src: Path, dst: Path):
    """Every float weight of 1024 values or more stored as fp16, followed by a Cast to fp32 under its old name."""
    import onnx
    from onnx import TensorProto, helper, numpy_helper

    m = onnx.load(str(src))
    g = m.graph
    inits, casts = [], []
    for t in g.initializer:
        a = numpy_helper.to_array(t)
        if t.data_type != TensorProto.FLOAT or a.size < 1024:
            inits.append(t)
            continue
        inits.append(numpy_helper.from_array(a.astype(np.float16), t.name + "__q"))
        casts.append(helper.make_node("Cast", [t.name + "__q"], [t.name], to=TensorProto.FLOAT))
    del g.initializer[:]
    g.initializer.extend(inits)
    nodes = casts + list(g.node)
    del g.node[:]
    g.node.extend(nodes)
    onnx.checker.check_model(m)
    onnx.save(m, str(dst))


def fuse_attention(src: Path, dst: Path, head_dim: int = 32):
    """Each spelled-out attention - Softmax(MatMul(q * sqrt(s), Transpose(k) * sqrt(s))) read by one MatMul with v -
    becomes Attention(q, k, v, scale = s), s = 1 / sqrt(head_dim); what only computed the scaling goes. Every
    pattern must match exactly, or nothing is written."""
    import onnx
    from onnx import helper

    m = onnx.load(str(src))
    g = m.graph
    prod = {o: n for n in g.node for o in n.output}
    readers = {}
    for n in g.node:
        for i in n.input:
            readers.setdefault(i, []).append(n)
    gone, fused = set(), {}
    softmaxes = [n for n in g.node if n.op_type == "Softmax"]
    for sm in softmaxes:
        scores = prod[sm.input[0]]
        after = readers[sm.output[0]]
        if scores.op_type != "MatMul" or len(after) != 1 or after[0].op_type != "MatMul":
            sys.exit(f"{sm.name}: not the attention pattern")
        mq, mk = prod[scores.input[0]], prod[scores.input[1]]
        tk = prod.get(mk.input[0])
        perm = [list(helper.get_attribute_value(a)) for a in tk.attribute if a.name == "perm"] if tk else []
        is_transpose = tk is not None and tk.op_type == "Transpose" and perm == [[0, 1, 3, 2]]
        if mq.op_type != "Mul" or mk.op_type != "Mul" or not is_transpose:
            sys.exit(f"{sm.name}: not the attention pattern")
        out = after[0]
        fused[out.output[0]] = helper.make_node(
            "Attention", [mq.input[0], tk.input[0], out.input[1]], [out.output[0]], name=sm.name + ".fused",
            scale=float(1.0 / np.sqrt(head_dim)),
        )
        gone.update(id(n) for n in (sm, scores, out, mq, mk, tk))
    nodes = []
    for n in g.node:
        if id(n) in gone:
            if n.output[0] in fused:
                nodes.append(fused[n.output[0]])
            continue
        nodes.append(n)
    outputs = {o.name for o in g.output}
    while True:
        read = {i for n in nodes for i in n.input} | outputs
        kept = [n for n in nodes if any(o in read for o in n.output)]
        if len(kept) == len(nodes):
            break
        nodes = kept
    del g.node[:]
    g.node.extend(nodes)
    onnx.save(m, str(dst))
    print(f"{len(softmaxes)} attentions fused")


def test_input():
    """A spectrogram-like input: log-mel values run from 0 (silence) to about 7."""
    rng = np.random.default_rng(0)
    return (rng.random((1, FRAMES, MELS)) * 6.0).astype(np.float32)


def run_onnx(path: Path, x):
    import onnxruntime as ort

    so = ort.SessionOptions()
    so.intra_op_num_threads = 1
    s = ort.InferenceSession(str(path), so, providers=["CPUExecutionProvider"])
    b, d = s.run(None, {s.get_inputs()[0].name: x})
    return b.ravel(), d.ravel()


def compare(name, ref, got):
    diff = max(float(np.abs(ref[0] - got[0]).max()), float(np.abs(ref[1] - got[1]).max()))
    flips = int(((ref[0] > 0) != (got[0] > 0)).sum() + ((ref[1] > 0) != (got[1] > 0)).sum())
    print(f"{name}: largest logit difference {diff:.4f}, frames on the other side of zero {flips}")
    return diff < 0.05 and flips <= 2


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--beat-this", required=True, type=Path, help="a checkout of github.com/CPJKU/beat_this")
    ap.add_argument("--checkpoint", type=Path, help="small0.ckpt, if already downloaded")
    ap.add_argument("--out", required=True, type=Path)
    args = ap.parse_args()

    import torch

    with tempfile.TemporaryDirectory() as tmp:
        ckpt = args.checkpoint or Path(tmp) / "small0.ckpt"
        if not args.checkpoint:
            print("fetching", CHECKPOINT_URL)
            urllib.request.urlretrieve(CHECKPOINT_URL, ckpt)
        if sha256(ckpt) != CHECKPOINT_SHA256:
            sys.exit(f"{ckpt} is not the published small0 checkpoint (SHA-256 {sha256(ckpt)})")
        model, hparams = load_model(args.beat_this, ckpt)
        print("small0:", hparams, sum(p.numel() for p in model.parameters()), "parameters")
        fp32, fp16 = Path(tmp) / "small0-fp32.onnx", Path(tmp) / "small0-fp16.onnx"
        # Exported before the model has run: a run first leaves the rotary embedding's cache filled, and the file
        # then carries it as constants (180 kB more, the same logits).
        export(model, fp32)
        to_fp16_weights(fp32, fp16)
        # The reference from a model loaded again: tracing leaves the traced sizes in that cache, and the traced
        # model itself then answers differently (logits off by 5); the ONNX file is right.
        model, _ = load_model(args.beat_this, ckpt)
        x = test_input()
        with torch.no_grad():
            out = model(torch.from_numpy(x))
        ref = (out["beat"].numpy().ravel(), out["downbeat"].numpy().ravel())
        ok = compare("fp16 export against PyTorch", ref, run_onnx(fp16, x))
        fuse_attention(fp16, args.out)
    print(f"{args.out}: {args.out.stat().st_size} bytes, SHA-256 {sha256(args.out)}")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
