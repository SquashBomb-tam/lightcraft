//! `cargo xtask models [--download]`: the AI mask models (see `models/README.md`).
//!
//! The upstream weight files are downloaded into `models/download/` (git-ignored), checked against
//! their pinned SHA-256, and converted into the files the app loads (`models/*.safetensors`):
//! batch norms folded into convolutions, U²-Net layers named, the sky model's ONNX weights mapped
//! onto U²-Net's layers by execution order (every shape checked), U²-Net weights stored as f16.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use candle_core::{DType, Device, Tensor};
use sha2::{Digest, Sha256};

/// An upstream weight file: where it comes from and what it must hash to.
struct Source {
    file: &'static str,
    url: &'static str,
    sha256: &'static str,
    size: u64,
}

/// MobileSAM, Apache-2.0 (github.com/ChaoningZhang/MobileSAM, `weights/mobile_sam.pt`).
const OBJECT: Source = Source {
    file: "mobile_sam.pt",
    url: "https://raw.githubusercontent.com/ChaoningZhang/MobileSAM/c12dd83cbe26dffdcc6a0f9e7be2f6fb024df0ed/weights/mobile_sam.pt",
    sha256: "6dbb90523a35330fedd7f1d3dfc66f995213d81b29a5ca8108dbcdd4e37d6c2f",
    size: 40_728_226,
};

/// U²-Net (full), Apache-2.0 (github.com/xuebinqin/U-2-Net `u2net.pth`; byte-identical mirror).
const SUBJECT: Source = Source {
    file: "u2net.pth",
    url: "https://huggingface.co/Carve/u2net-universal/resolve/10305d785481cf4b2eee1d447c39cd6e5f43d74b/full_weights.pth",
    sha256: "10025a17f49cd3208afc342b589890e402ee63123d6f2d289a4a0903695cce58",
    size: 176_290_937,
};

/// Sky segmentation (U²-Net trained on sky), MIT (github.com/xiongzhu666/Sky-Segmentation-and-Post-processing).
const SKY: Source = Source {
    file: "skyseg.onnx",
    url: "https://huggingface.co/JianyuanWang/skyseg/resolve/3ba8c6df1d9ba9ff26f637c7ba9568ac11a9aa7f/skyseg.onnx",
    sha256: "ab9c34c64c3d821220a2886a4a06da4642ffa14d5b30e8d5339056a089aa1d39",
    size: 175_997_079,
};

pub fn run(root: &Path, args: &[&str]) -> Result<(), String> {
    let dir = match args.iter().position(|a| *a == "--dir") {
        Some(i) => PathBuf::from(args.get(i + 1).ok_or("--dir needs a path")?),
        None => root.join("models"),
    };
    let download = args.contains(&"--download");
    let raw = dir.join("download");
    std::fs::create_dir_all(&raw).map_err(|e| format!("create {}: {e}", raw.display()))?;
    for s in [&OBJECT, &SUBJECT, &SKY] {
        let path = raw.join(s.file);
        if verified(&path, s)? {
            continue;
        }
        if !download {
            return Err(format!("{} is missing or doesn't match its pinned SHA-256: run `cargo xtask models --download`", path.display()));
        }
        let tmp = path.with_extension("part");
        let mut curl = Command::new("curl");
        curl.args(["-fsSL", "--retry", "3", "-o"]).arg(&tmp).arg(s.url);
        crate::run(curl, &format!("curl {}", s.url))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
        if !verified(&path, s)? {
            let _ = std::fs::remove_file(&path);
            return Err(format!("{}: the download doesn't match the pinned SHA-256 {}", s.url, s.sha256));
        }
    }
    convert_object(&raw.join(OBJECT.file), &dir.join("object.safetensors"))?;
    convert_subject(&raw.join(SUBJECT.file), &dir.join("subject.safetensors"))?;
    convert_sky(&raw.join(SKY.file), &dir.join("sky.safetensors"))?;
    for name in ["object", "subject", "sky"] {
        let p = dir.join(format!("{name}.safetensors"));
        let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        println!("{:<26} {:>6.1} MB", p.file_name().unwrap_or_default().to_string_lossy(), size as f64 / 1e6);
    }
    Ok(())
}

/// Whether `path` exists with `s`'s size and SHA-256.
fn verified(path: &Path, s: &Source) -> Result<bool, String> {
    match std::fs::metadata(path) {
        Ok(m) if m.len() == s.size => {}
        _ => return Ok(false),
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(hex(&Sha256::digest(&bytes)) == s.sha256)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn save(tensors: &HashMap<String, Tensor>, out: &Path) -> Result<(), String> {
    let tmp = out.with_extension("part");
    candle_core::safetensors::save(tensors, &tmp).map_err(|e| format!("{}: {e}", out.display()))?;
    std::fs::rename(&tmp, out).map_err(|e| format!("{}: {e}", out.display()))
}

/// MobileSAM: the checkpoint's image encoder, prompt encoder and mask decoder, f32.
fn convert_object(src: &Path, out: &Path) -> Result<(), String> {
    let all = candle_core::pickle::read_all(src).map_err(|e| format!("{}: {e}", src.display()))?;
    let keep: HashMap<String, Tensor> = all
        .into_iter()
        .filter(|(k, _)| ["image_encoder.", "prompt_encoder.", "mask_decoder."].iter().any(|p| k.starts_with(p)))
        .map(|(k, t)| t.to_dtype(DType::F32).map(|t| (k, t)))
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    if keep.len() < 100 {
        return Err(format!("{}: only {} tensors look like MobileSAM's", src.display(), keep.len()));
    }
    save(&keep, out)
}

/// U²-Net from its PyTorch checkpoint: each REBNCONV's batch norm folded into its convolution.
fn convert_subject(src: &Path, out: &Path) -> Result<(), String> {
    let bytes = std::fs::read(src).map_err(|e| format!("{}: {e}", src.display()))?;
    let all = legacy_pth(&bytes).map_err(|e| format!("{}: {e}", src.display()))?;
    let get = |k: &str| all.get(k).cloned().ok_or_else(|| format!("{}: no tensor {k}", src.display()));
    let mut outt = HashMap::new();
    for l in lightcraft_segment::u2net_layers() {
        let (w, b) = if l.name.starts_with("stage") {
            let w = get(&format!("{}.conv_s1.weight", l.name))?;
            let b = get(&format!("{}.conv_s1.bias", l.name))?;
            let bn = |p: &str| get(&format!("{}.bn_s1.{p}", l.name));
            fold(&w, &b, &bn("weight")?, &bn("bias")?, &bn("running_mean")?, &bn("running_var")?).map_err(|e| format!("{}: {e}", l.name))?
        } else {
            (get(&format!("{}.weight", l.name))?, get(&format!("{}.bias", l.name))?)
        };
        check_shape(&l, &w, &b)?;
        insert_f16(&mut outt, &l.name, w, b)?;
    }
    save(&outt, out)
}

/// The magic number pickle that opens a legacy PyTorch checkpoint (protocol 2, LONG1
/// 0x1950a86a20f9469cfc6c, STOP).
const LEGACY_MAGIC: [u8; 15] = [0x80, 2, 0x8a, 10, 0x6c, 0xfc, 0x9c, 0x46, 0xf9, 0x20, 0x6a, 0xa8, 0x50, 0x19, b'.'];

/// The float tensors of a PyTorch checkpoint in the legacy (pre-1.6, not zipped) format, which
/// `candle_core::pickle` doesn't read: after the magic number come pickles of the protocol
/// version, the system info, the state dict and its storage keys, then each storage (in the keys'
/// order) as an i64 element count and its raw little-endian data.
fn legacy_pth(bytes: &[u8]) -> Result<HashMap<String, Tensor>, String> {
    use candle_core::pickle::{Object, Stack};
    use std::io::Cursor;

    fn pickle(r: &mut Cursor<&[u8]>) -> Result<Object, String> {
        let mut s = Stack::empty();
        s.read_loop(r).map_err(|e| format!("pickle: {e}"))?;
        s.finalize().map_err(|e| format!("pickle: {e}"))
    }
    let bad = |what: &str, o: Object| {
        let o = format!("{o:?}");
        format!("unexpected {what}: {}", o.get(..o.char_indices().nth(200).map_or(o.len(), |(i, _)| i)).unwrap_or_default())
    };
    let rest = bytes.strip_prefix(&LEGACY_MAGIC[..]).ok_or("not a legacy PyTorch checkpoint (no magic number)")?;
    let mut r = Cursor::new(rest);
    pickle(&mut r)?; // protocol version
    let info = pickle(&mut r)?.dict().map_err(|o| bad("system info", o))?;
    if !info.iter().any(|(k, v)| *k == Object::Unicode("little_endian".into()) && *v == Object::Bool(true)) {
        return Err("saved on a big-endian machine".into());
    }
    let state = pickle(&mut r)?.dict().map_err(|o| bad("state dict", o))?;
    let keys: Vec<String> = match pickle(&mut r)? {
        Object::List(k) | Object::Tuple(k) => k.into_iter().map(|k| k.unicode().map_err(|o| bad("storage key", o))).collect::<Result<_, _>>()?,
        o => return Err(bad("storage keys", o)),
    };

    // each tensor: _rebuild_tensor_v2((storage id), offset, size, stride, ...), the storage id
    // being ('storage', <type>Storage, key, location, element count, ...)
    struct View {
        name: String,
        key: String,
        float: bool,
        offset: usize,
        size: Vec<usize>,
        stride: Vec<usize>,
    }
    let mut views = Vec::new();
    let mut elem: HashMap<String, usize> = HashMap::new();
    for (name, value) in state {
        let name = name.unicode().map_err(|o| bad("tensor name", o))?;
        if name == "_metadata" {
            // the state dict's attributes (module versions), merged in by the unpickler
            continue;
        }
        let (callable, args) = value.reduce().map_err(|o| bad(&name, o))?;
        if callable != (Object::Class { module_name: "torch._utils".into(), class_name: "_rebuild_tensor_v2".into() }) {
            return Err(format!("{name}: unsupported tensor constructor {callable:?}"));
        }
        let mut a = args.tuple().map_err(|o| bad(&name, o))?;
        if a.len() < 4 {
            return Err(format!("{name}: {} rebuild arguments", a.len()));
        }
        let stride = Vec::<usize>::try_from(a.remove(3)).map_err(|o| bad(&name, o))?;
        let size = Vec::<usize>::try_from(a.remove(2)).map_err(|o| bad(&name, o))?;
        let offset = a.remove(1).int_or_long().map_err(|o| bad(&name, o))? as usize;
        let mut id = a.remove(0).persistent_load().and_then(Object::tuple).map_err(|o| bad(&name, o))?;
        if id.len() < 5 {
            return Err(format!("{name}: storage id of {} fields", id.len()));
        }
        let (_, class) = id.remove(1).class().map_err(|o| bad(&name, o))?;
        let key = id.remove(1).unicode().map_err(|o| bad(&name, o))?;
        let size_bytes = match class.as_str() {
            "FloatStorage" | "IntStorage" => 4,
            "DoubleStorage" | "LongStorage" => 8,
            "HalfStorage" | "BFloat16Storage" | "ShortStorage" => 2,
            "ByteStorage" | "CharStorage" | "BoolStorage" => 1,
            other => return Err(format!("{name}: unsupported storage {other}")),
        };
        elem.insert(key.clone(), size_bytes);
        views.push(View { name, key, float: class == "FloatStorage", offset, size, stride });
    }

    let mut data: HashMap<String, &[u8]> = HashMap::new();
    let mut at = r.position() as usize;
    for key in keys {
        let n = rest.get(at..at + 8).ok_or_else(|| format!("storage {key}: truncated"))?;
        let n = i64::from_le_bytes(n.try_into().map_err(|_| "count")?);
        let e = *elem.get(&key).ok_or_else(|| format!("storage {key} isn't used by any tensor"))?;
        let len = usize::try_from(n).ok().and_then(|n| n.checked_mul(e)).ok_or_else(|| format!("storage {key}: {n} elements"))?;
        let body = rest.get(at + 8..at + 8 + len).ok_or_else(|| format!("storage {key}: truncated"))?;
        data.insert(key, body);
        at += 8 + len;
    }
    if at != rest.len() {
        return Err(format!("{} bytes after the last storage", rest.len() - at));
    }

    let mut out = HashMap::new();
    for v in views.into_iter().filter(|v| v.float) {
        // row-major and contiguous, the only layout a saved state dict uses
        let mut want = vec![1usize; v.size.len()];
        for i in (0..v.size.len().saturating_sub(1)).rev() {
            want[i] = want[i + 1] * v.size[i + 1];
        }
        if v.stride != want {
            return Err(format!("{}: non-contiguous strides {:?}", v.name, v.stride));
        }
        let n: usize = v.size.iter().product();
        let raw = data[&v.key].get(4 * v.offset..4 * (v.offset + n)).ok_or_else(|| format!("{}: past the end of its storage", v.name))?;
        let vals: Vec<f32> = raw.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect();
        let t = Tensor::from_vec(vals, v.size, &Device::Cpu).map_err(|e| format!("{}: {e}", v.name))?;
        out.insert(v.name, t);
    }
    Ok(out)
}

/// The sky model from its ONNX export (batch norms already folded): Conv nodes in graph order are
/// U²-Net's layers in execution order; every weight shape and dilation is checked against the
/// architecture before anything is written.
fn convert_sky(src: &Path, out: &Path) -> Result<(), String> {
    let bytes = std::fs::read(src).map_err(|e| format!("{}: {e}", src.display()))?;
    let model = onnx::parse(&bytes).map_err(|e| format!("{}: {e}", src.display()))?;
    let layers = lightcraft_segment::u2net_layers();
    let convs: Vec<&onnx::Node> = model.nodes.iter().filter(|n| n.op_type == "Conv").collect();
    if convs.len() != layers.len() {
        return Err(format!("{}: {} Conv nodes, U²-Net has {}", src.display(), convs.len(), layers.len()));
    }
    let mut outt = HashMap::new();
    for (l, n) in layers.iter().zip(convs) {
        let t = |i: usize| -> Result<Tensor, String> {
            let name = n.inputs.get(i).ok_or_else(|| format!("{}: Conv without input {i}", l.name))?;
            let init = model.initializers.get(name).ok_or_else(|| format!("{}: no initializer {name}", l.name))?;
            Tensor::from_vec(init.data.clone(), init.dims.clone(), &Device::Cpu).map_err(|e| e.to_string())
        };
        let dil = n.ints.get("dilations").and_then(|d| d.first()).copied().unwrap_or(1) as usize;
        if dil != l.dilation {
            return Err(format!("{}: dilation {dil} in the ONNX file, {} in U²-Net", l.name, l.dilation));
        }
        let (w, b) = (t(1)?, t(2)?);
        check_shape(l, &w, &b)?;
        insert_f16(&mut outt, &l.name, w, b)?;
    }
    save(&outt, out)
}

/// `w`, `b` of a convolution followed by batch norm (γ, β, mean, var; ε = 1e-5) as one convolution.
fn fold(w: &Tensor, b: &Tensor, gamma: &Tensor, beta: &Tensor, mean: &Tensor, var: &Tensor) -> candle_core::Result<(Tensor, Tensor)> {
    let scale = (gamma / (var + 1e-5)?.sqrt()?)?;
    let w = w.broadcast_mul(&scale.reshape(((), 1, 1, 1))?)?;
    let b = ((b - mean)? * &scale)?.add(beta)?;
    Ok((w, b))
}

fn check_shape(l: &lightcraft_segment::U2NetLayer, w: &Tensor, b: &Tensor) -> Result<(), String> {
    let want = [l.out_ch, l.in_ch, l.kernel, l.kernel];
    if w.dims() != want || b.dims() != [l.out_ch] {
        return Err(format!("{}: weight {:?} / bias {:?}, expected {want:?} / [{}]", l.name, w.dims(), b.dims(), l.out_ch));
    }
    Ok(())
}

fn insert_f16(out: &mut HashMap<String, Tensor>, name: &str, w: Tensor, b: Tensor) -> Result<(), String> {
    let f16 = |t: Tensor| t.to_dtype(DType::F32).and_then(|t| t.to_dtype(DType::F16)).map_err(|e| e.to_string());
    out.insert(format!("{name}.weight"), f16(w)?);
    out.insert(format!("{name}.bias"), f16(b)?);
    Ok(())
}

/// Just enough of ONNX (protobuf) to read a graph's nodes and float initializers.
mod onnx {
    use std::collections::HashMap;

    pub struct Node {
        pub op_type: String,
        pub inputs: Vec<String>,
        /// Integer-list attributes (`dilations`, `pads`, …).
        pub ints: HashMap<String, Vec<i64>>,
    }

    pub struct Init {
        pub dims: Vec<usize>,
        pub data: Vec<f32>,
    }

    pub struct Model {
        pub nodes: Vec<Node>,
        pub initializers: HashMap<String, Init>,
    }

    /// A protobuf field: number and value.
    enum Val<'a> {
        Int(u64),
        Bytes(&'a [u8]),
        Fixed32(u32),
    }

    fn fields(mut b: &[u8]) -> Result<Vec<(u32, Val<'_>)>, String> {
        let mut out = Vec::new();
        while !b.is_empty() {
            let key = varint(&mut b)?;
            let (num, wire) = ((key >> 3) as u32, key & 7);
            out.push((
                num,
                match wire {
                    0 => Val::Int(varint(&mut b)?),
                    1 => {
                        let (v, rest) = b.split_at_checked(8).ok_or("truncated fixed64")?;
                        b = rest;
                        Val::Int(u64::from_le_bytes(v.try_into().map_err(|_| "fixed64")?))
                    }
                    2 => {
                        let n = varint(&mut b)? as usize;
                        let (v, rest) = b.split_at_checked(n).ok_or("truncated field")?;
                        b = rest;
                        Val::Bytes(v)
                    }
                    5 => {
                        let (v, rest) = b.split_at_checked(4).ok_or("truncated fixed32")?;
                        b = rest;
                        Val::Fixed32(u32::from_le_bytes(v.try_into().map_err(|_| "fixed32")?))
                    }
                    w => return Err(format!("unsupported wire type {w}")),
                },
            ));
        }
        Ok(out)
    }

    fn varint(b: &mut &[u8]) -> Result<u64, String> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let (&x, rest) = b.split_first().ok_or("truncated varint")?;
            *b = rest;
            v |= ((x & 0x7f) as u64) << shift;
            if x & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err("varint too long".into())
    }

    fn string(v: &Val<'_>) -> String {
        match v {
            Val::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            _ => String::new(),
        }
    }

    /// Repeated int64s, packed or not.
    fn ints(v: &Val<'_>, out: &mut Vec<i64>) -> Result<(), String> {
        match v {
            Val::Int(i) => out.push(*i as i64),
            Val::Bytes(b) => {
                let mut b = *b;
                while !b.is_empty() {
                    out.push(varint(&mut b)? as i64);
                }
            }
            Val::Fixed32(_) => return Err("unexpected fixed32 in an int list".into()),
        }
        Ok(())
    }

    pub fn parse(bytes: &[u8]) -> Result<Model, String> {
        // ModelProto.graph = 7; GraphProto.node = 1, .initializer = 5
        let graph = fields(bytes)?.into_iter().find_map(|(n, v)| if let (7, Val::Bytes(b)) = (n, v) { Some(b) } else { None }).ok_or("no graph")?;
        let mut model = Model { nodes: Vec::new(), initializers: HashMap::new() };
        for (num, v) in fields(graph)? {
            let Val::Bytes(b) = v else { continue };
            match num {
                1 => model.nodes.push(node(b)?),
                5 => {
                    let (name, init) = tensor(b)?;
                    model.initializers.insert(name, init);
                }
                _ => {}
            }
        }
        Ok(model)
    }

    // NodeProto: input = 1, op_type = 4, attribute = 5; AttributeProto: name = 1, ints = 8
    fn node(b: &[u8]) -> Result<Node, String> {
        let mut n = Node { op_type: String::new(), inputs: Vec::new(), ints: HashMap::new() };
        for (num, v) in fields(b)? {
            match num {
                1 => n.inputs.push(string(&v)),
                4 => n.op_type = string(&v),
                5 => {
                    let Val::Bytes(a) = v else { continue };
                    let (mut name, mut list) = (String::new(), Vec::new());
                    for (an, av) in fields(a)? {
                        match an {
                            1 => name = string(&av),
                            8 => ints(&av, &mut list)?,
                            _ => {}
                        }
                    }
                    if !list.is_empty() {
                        n.ints.insert(name, list);
                    }
                }
                _ => {}
            }
        }
        Ok(n)
    }

    // TensorProto: dims = 1, data_type = 2 (1 = float), float_data = 4, name = 8, raw_data = 9
    fn tensor(b: &[u8]) -> Result<(String, Init), String> {
        let (mut name, mut dims, mut dtype, mut data) = (String::new(), Vec::new(), 0u64, Vec::new());
        for (num, v) in fields(b)? {
            match num {
                1 => {
                    let mut d = Vec::new();
                    ints(&v, &mut d)?;
                    dims.extend(d.into_iter().map(|x| x as usize));
                }
                2 => {
                    if let Val::Int(t) = v {
                        dtype = t;
                    }
                }
                4 => match v {
                    Val::Fixed32(x) => data.push(f32::from_bits(x)),
                    Val::Bytes(raw) => data.extend(raw.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c))),
                    Val::Int(_) => return Err("float_data as varint".into()),
                },
                8 => name = string(&v),
                9 => {
                    if let Val::Bytes(raw) = v {
                        data.extend(raw.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)));
                    }
                }
                _ => {}
            }
        }
        if dtype != 1 {
            // only float tensors are weights here; others (shapes, scales) are skipped
            data.clear();
        }
        if dtype == 1 && data.len() != dims.iter().product::<usize>() {
            return Err(format!("initializer {name}: {} values for dims {dims:?}", data.len()));
        }
        Ok((name, Init { dims, data }))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Encode a varint (test helper).
        fn v(mut x: u64, out: &mut Vec<u8>) {
            loop {
                let b = (x & 0x7f) as u8;
                x >>= 7;
                if x == 0 {
                    out.push(b);
                    break;
                }
                out.push(b | 0x80);
            }
        }

        fn field(num: u64, wire: u64, payload: &[u8], out: &mut Vec<u8>) {
            v(num << 3 | wire, out);
            if wire == 2 {
                v(payload.len() as u64, out);
            }
            out.extend_from_slice(payload);
        }

        #[test]
        fn parses_a_tiny_conv_graph() {
            // tensor "w": dims [1, 2] (packed), float, raw data [1.5, -2.0]
            let mut t = Vec::new();
            let mut dims = Vec::new();
            v(1, &mut dims);
            v(2, &mut dims);
            field(1, 2, &dims, &mut t);
            field(2, 0, &[1], &mut t);
            field(8, 2, b"w", &mut t);
            let raw: Vec<u8> = [1.5f32, -2.0].iter().flat_map(|f| f.to_le_bytes()).collect();
            field(9, 2, &raw, &mut t);
            // node: Conv(x, w) with dilations [2, 2]
            let mut a = Vec::new();
            field(1, 2, b"dilations", &mut a);
            let mut d = Vec::new();
            v(2, &mut d);
            v(2, &mut d);
            field(8, 2, &d, &mut a);
            let mut n = Vec::new();
            field(1, 2, b"x", &mut n);
            field(1, 2, b"w", &mut n);
            field(4, 2, b"Conv", &mut n);
            field(5, 2, &a, &mut n);
            let mut g = Vec::new();
            field(1, 2, &n, &mut g);
            field(5, 2, &t, &mut g);
            let mut m = Vec::new();
            field(1, 0, &[7], &mut m); // ir_version, ignored
            field(7, 2, &g, &mut m);
            let model = parse(&m).unwrap();
            assert_eq!(model.nodes.len(), 1);
            assert_eq!(model.nodes[0].op_type, "Conv");
            assert_eq!(model.nodes[0].inputs, ["x", "w"]);
            assert_eq!(model.nodes[0].ints["dilations"], [2, 2]);
            let w = &model.initializers["w"];
            assert_eq!((w.dims.clone(), w.data.clone()), (vec![1, 2], vec![1.5, -2.0]));
        }

        #[test]
        fn rejects_garbage() {
            assert!(parse(&[0xff, 0xff, 0xff]).is_err());
            assert!(parse(&[]).is_err(), "no graph");
            // a length running past the end
            assert!(parse(&[7 << 3 | 2, 50, 1, 2]).is_err());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding_matches_conv_then_batch_norm() {
        let d = Device::Cpu;
        let x = Tensor::randn(0f32, 1.0, (1, 2, 5, 5), &d).unwrap();
        let w = Tensor::randn(0f32, 1.0, (3, 2, 3, 3), &d).unwrap();
        let b = Tensor::randn(0f32, 1.0, 3, &d).unwrap();
        let (g, beta, mean) = (
            Tensor::new(&[1.5f32, 0.5, 2.0], &d).unwrap(),
            Tensor::new(&[0.1f32, -0.2, 0.3], &d).unwrap(),
            Tensor::new(&[0.4f32, 0.0, -1.0], &d).unwrap(),
        );
        let var = Tensor::new(&[0.25f32, 1.0, 4.0], &d).unwrap();
        let c = |t: &Tensor| t.reshape((1, 3, 1, 1)).unwrap();
        let reference = x.conv2d(&w, 1, 1, 1, 1).unwrap().broadcast_add(&c(&b)).unwrap();
        let reference = reference.broadcast_sub(&c(&mean)).unwrap().broadcast_div(&c(&(&var + 1e-5).unwrap().sqrt().unwrap())).unwrap();
        let reference = reference.broadcast_mul(&c(&g)).unwrap().broadcast_add(&c(&beta)).unwrap();
        let (fw, fb) = fold(&w, &b, &g, &beta, &mean, &var).unwrap();
        let ours = x.conv2d(&fw, 1, 1, 1, 1).unwrap().broadcast_add(&c(&fb)).unwrap();
        let diff: f32 = (ours - reference).unwrap().abs().unwrap().max_all().unwrap().to_scalar().unwrap();
        assert!(diff < 1e-4, "{diff}");
    }

    /// Pickle helpers (protocol 2) for a hand-built legacy checkpoint.
    fn unicode(s: &str, out: &mut Vec<u8>) {
        out.push(b'X');
        out.extend((s.len() as u32).to_le_bytes());
        out.extend(s.as_bytes());
    }

    fn ints(v: &[u8], out: &mut Vec<u8>) {
        out.push(b'(');
        for x in v {
            out.extend([b'K', *x]);
        }
        out.push(b't');
    }

    /// A legacy checkpoint: `w` (2 × 3) and `v` (a 3-element view at offset 3) over one 6-float
    /// storage, plus `extra` floats after the storage and the system info's endianness.
    fn legacy(little_endian: bool, extra: &[u8]) -> Vec<u8> {
        let mut b = LEGACY_MAGIC.to_vec();
        b.extend(b"\x80\x02M\xe9\x03."); // protocol version 1001
        b.extend(b"\x80\x02}(");
        unicode("little_endian", &mut b);
        b.push(if little_endian { 0x88 } else { 0x89 });
        b.extend(b"u.");
        // the state dict
        b.extend(b"\x80\x02ccollections\nOrderedDict\n)R(");
        for (name, offset, size, stride) in [("w", 0u8, &[2u8, 3][..], &[3u8, 1][..]), ("v", 3, &[3][..], &[1][..])] {
            unicode(name, &mut b);
            b.extend(b"ctorch._utils\n_rebuild_tensor_v2\n(");
            b.push(b'(');
            unicode("storage", &mut b);
            b.extend(b"ctorch\nFloatStorage\n");
            unicode("k1", &mut b);
            unicode("cpu", &mut b);
            b.extend([b'K', 6, b't', b'Q']);
            b.extend([b'K', offset]);
            ints(size, &mut b);
            ints(stride, &mut b);
            b.extend(b"\x89Nt");
            b.push(b'R');
        }
        b.extend(b"u.");
        // the storage keys, then the storage: count and little-endian floats
        b.extend(b"\x80\x02](");
        unicode("k1", &mut b);
        b.extend(b"e.");
        b.extend(6i64.to_le_bytes());
        for i in 0..6 {
            b.extend((i as f32 * 1.5).to_le_bytes());
        }
        b.extend(extra);
        b
    }

    #[test]
    fn reads_legacy_pytorch_checkpoints() {
        let t = legacy_pth(&legacy(true, &[])).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t["w"].dims(), [2, 3]);
        assert_eq!(t["w"].to_vec2::<f32>().unwrap(), [[0.0, 1.5, 3.0], [4.5, 6.0, 7.5]]);
        assert_eq!(t["v"].to_vec1::<f32>().unwrap(), [4.5, 6.0, 7.5], "a view at an offset into the shared storage");
        // damaged or foreign files are refused, never misread
        let good = legacy(true, &[]);
        assert!(legacy_pth(&good[1..]).is_err(), "no magic number");
        assert!(legacy_pth(&good[..good.len() - 5]).is_err(), "a truncated storage");
        assert!(legacy_pth(&legacy(true, &[0, 0, 0, 0])).is_err(), "bytes after the last storage");
        assert!(legacy_pth(&legacy(false, &[])).is_err(), "big-endian data");
        assert!(legacy_pth(b"PK\x03\x04 a zip checkpoint").is_err());
        assert!(legacy_pth(&[]).is_err());
    }

    #[test]
    fn hashes_are_hex_sha256() {
        assert_eq!(hex(&Sha256::digest(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
