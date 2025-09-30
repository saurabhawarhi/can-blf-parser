// ###############################################################
// lib.rs
// can-blf-parser (WASM)
// Final consolidated version (smart preview, streaming CSV, decimation)
// Sections numbered and commented as requested
// ###############################################################

/*
SECTION 0: Brief overview
- Exports BlfSession (WASM-visible).
- Supports multi-DBC (array of DBC texts) mapped to channels (u8 array).
- Smart preview: limit by file size (<=20MB -> full slice, >20MB -> 5% slice max 100MB).
- Preview frame caps: preview max 50 frames with signals; if total <=100k frames parse full
slice (still capped by slice size).
- Streaming CSV export and decimated_stream parse the full provided BLF buffer and stream
results (they accept a JS progress callback).
- Signal names are channel-tagged as "CAN{channel}.{SignalName}" to avoid collisions.
*/

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsValue;

use serde::Serialize;
use serde_json::json;
use serde_wasm_bindgen;

use std::collections::HashMap;
use std::io::Cursor;

use ablf::{BlfFile, ObjectTypes};
use can_dbc::{DBC, Signal, ByteOrder, ValueType};

use js_sys::Function;

// -------------------------------
// SECTION 1: Data structures returned to JS (serde-serializable)
// -------------------------------
#[derive(Serialize, Debug, Clone)]
pub struct SignalRow {
    pub signal: String, // "CAN{channel}.{SignalName}"
    pub value: f64,
    pub unit: String,
}

#[derive(Serialize, Debug, Clone)]
pub struct FrameRow {
    pub timestamp: f64,
    pub channel: String, // e.g., "CAN1"
    pub id: u32,
    pub name: String,
    pub event_type: String,
    pub dir: String,
    pub dlc: u8,
    pub data: Vec<u8>,
    pub signals: Vec<SignalRow>,
}

// -------------------------------
// SECTION 2: BlfSession (WASM-visible)
// -------------------------------
#[wasm_bindgen]
pub struct BlfSession {
    frames: Vec<FrameRow>,
    signal_names: Vec<String>,
}

#[wasm_bindgen]
impl BlfSession {
    // ---------------------------
    // 2.1 Constructor
    // ---------------------------
    #[wasm_bindgen(constructor)]
    pub fn new(
        blf_bytes: &[u8],
        dbc_texts: JsValue,
        channel_map: JsValue,
    ) -> Result<BlfSession, JsValue> {
        // Deserialize input JS arrays into Rust types
        let dbc_texts_vec: Vec<String> = serde_wasm_bindgen::from_value(dbc_texts)
            .map_err(|e| JsValue::from_str(&format!("dbc_texts must be array of strings: {:?}", e)))?;
        let channel_map_vec: Vec<u8> = serde_wasm_bindgen::from_value(channel_map)
            .map_err(|e| JsValue::from_str(&format!("channel_map must be array of u8: {:?}", e)))?;

        if dbc_texts_vec.len() != channel_map_vec.len() {
            return Err(JsValue::from_str("dbc_texts and channel_map must have same length"));
        }

        // Build DBC map: channel -> DBC
        let mut dbc_map: HashMap<u8, DBC> = HashMap::new();
        for (text, chan) in dbc_texts_vec.iter().zip(channel_map_vec.iter()) {
            let dbc = DBC::try_from(text.as_str())
                .map_err(|e| JsValue::from_str(&format!("Failed to parse DBC for channel {}: {:?}", chan, e)))?;
            dbc_map.insert(*chan, dbc);
        }

        // Create BlfFile reader
        let cursor = Cursor::new(blf_bytes);
        let blf = BlfFile::from_reader(cursor)
            .map_err(|(e, _)| JsValue::from_str(&format!("Failed to parse BLF: {:?}", e)))?;

        let mut frames: Vec<FrameRow> = Vec::new();
        let mut seen_signals: Vec<String> = Vec::new();

        // Iterate and build frames
        for obj in blf {
            if let Some(frame) = frame_from_obj(&obj.data, &dbc_map, Some(&mut seen_signals)) {
                frames.push(frame);
            }
        }

        seen_signals.sort();
        Ok(BlfSession { frames, signal_names: seen_signals })
    }

    // ---------------------------
    // 2.2 stats()
    // ---------------------------
    #[wasm_bindgen(js_name = stats)]
    pub fn stats(&self) -> Result<JsValue, JsValue> {
        let count = self.frames.len() as u32;
        let (first, last) = if let (Some(f), Some(l)) = (self.frames.first(), self.frames.last()) {
            (f.timestamp, l.timestamp)
        } else {
            (0.0, 0.0)
        };
        let sig_count = self.signal_names.len() as u32;
        serde_wasm_bindgen::to_value(&(count, first, last, sig_count))
            .map_err(|e| JsValue::from_str(&format!("serde failed: {:?}", e)))
    }

    // ---------------------------
    // 2.3 preview()
    // ---------------------------
    #[wasm_bindgen(js_name = preview)]
    pub fn preview(&self, n: usize) -> Result<JsValue, JsValue> {
        let take_n = std::cmp::min(n, self.frames.len());
        serde_wasm_bindgen::to_value(&self.frames[0..take_n])
            .map_err(|e| JsValue::from_str(&format!("serde failed: {:?}", e)))
    }

    // ---------------------------
    // 2.4 signals()
    // ---------------------------
    #[wasm_bindgen(js_name = signals)]
    pub fn signals(&self) -> Result<JsValue, JsValue> {
        serde_wasm_bindgen::to_value(&self.signal_names)
            .map_err(|e| JsValue::from_str(&format!("serde failed: {:?}", e)))
    }

    // ---------------------------
    // 2.5 decimated()
    // ---------------------------
    #[wasm_bindgen(js_name = decimated)]
    pub fn decimated(
        &self,
        max_points: usize,
        keep_signals: JsValue,
    ) -> Result<JsValue, JsValue> {
        let total = self.frames.len();
        if total == 0 {
            return serde_wasm_bindgen::to_value(&json!({"time": [], "signals": {}}))
                .map_err(|e| JsValue::from_str(&format!("serde failed: {:?}", e)));
        }

        let keep_opt: Option<Vec<String>> =
            if keep_signals.is_null() || keep_signals.is_undefined() {
                None
            } else {
                Some(serde_wasm_bindgen::from_value(keep_signals)
                    .map_err(|e| JsValue::from_str(&format!("keep_signals must be array of strings: {:?}", e)))?)
            };

        let keys: Vec<String> = keep_opt.unwrap_or_else(|| self.signal_names.clone());

        let step = std::cmp::max(1, total / max_points);
        let mut dec_time = Vec::new();
        let mut dec_signals: HashMap<String, Vec<Option<f64>>> =
            keys.iter().map(|k| (k.clone(), Vec::new())).collect();
        let mut last_seen: HashMap<String, Option<f64>> =
            keys.iter().map(|k| (k.clone(), None)).collect();

        for (i, frame) in self.frames.iter().enumerate() {
            for s in &frame.signals {
                last_seen.insert(s.signal.clone(), Some(s.value));
            }
            if i % step == 0 {
                dec_time.push(frame.timestamp);
                for k in &keys {
                    if let Some(arr) = dec_signals.get_mut(k) {
                        arr.push(last_seen.get(k).cloned().unwrap_or(None));
                    }
                }
            }
        }

        let mut out_signals = serde_json::Map::new();
        for (k, vec_opt) in dec_signals.into_iter() {
            let arr_values: Vec<serde_json::Value> = vec_opt
                .into_iter()
                .map(|o| o.map_or(serde_json::Value::Null, |v| json!(v)))
                .collect();
            out_signals.insert(k, serde_json::Value::Array(arr_values));
        }

        serde_wasm_bindgen::to_value(&json!({
            "time": dec_time,
            "signals": serde_json::Value::Object(out_signals)
        })).map_err(|e| JsValue::from_str(&format!("serde failed: {:?}", e)))
    }

    // ---------------------------
    // 2.6 export_csv()
    // ---------------------------
    #[wasm_bindgen(js_name = export_csv)]
    pub fn export_csv(&self, applied_signals: JsValue) -> Result<Vec<u8>, JsValue> {
        let selected: Option<Vec<String>> = if applied_signals.is_null() || applied_signals.is_undefined() {
            None
        } else {
            Some(serde_wasm_bindgen::from_value(applied_signals)
                .map_err(|e| JsValue::from_str(&format!("applied_signals must be array of strings: {:?}", e)))?)
        };

        let mut wtr = csv::WriterBuilder::new().has_headers(true).from_writer(vec![]);
        let mut header = vec![
            "Time [s]".to_string(),
            "Channel".to_string(),
            "ID".to_string(),
            "Name".to_string(),
            "Event Type".to_string(),
            "Dir".to_string(),
            "DLC".to_string(),
            "Data".to_string(),
        ];
        if let Some(ref sel) = selected {
            header.extend(sel.iter().cloned());
        }
        wtr.write_record(&header)
            .map_err(|e| JsValue::from_str(&format!("csv write failed: {:?}", e)))?;

        for f in &self.frames {
            let mut row: Vec<String> = vec![
                format!("{:.6}", f.timestamp),
                f.channel.clone(),
                format!("0x{:X}", f.id),
                f.name.clone(),
                f.event_type.clone(),
                f.dir.clone(),
                f.dlc.to_string(),
                f.data.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" "),
            ];

            let sig_map: HashMap<&str, f64> =
                f.signals.iter().map(|s| (s.signal.as_str(), s.value)).collect();

            if let Some(ref sel) = selected {
                for sname in sel {
                    row.push(sig_map.get(sname.as_str()).map_or(String::new(), |v| v.to_string()));
                }
            }

            wtr.write_record(&row)
                .map_err(|e| JsValue::from_str(&format!("csv write failed: {:?}", e)))?;
        }

        wtr.into_inner()
            .map_err(|e| JsValue::from_str(&format!("csv finalize failed: {:?}", e)))
    }

    // ---------------------------
    // 2.7 free_memory()
    // ---------------------------
    #[wasm_bindgen(js_name = free_memory)]
    pub fn free_memory(&mut self) {
        self.frames.clear();
        self.signal_names.clear();
    }

    // ---------------------------
    // 2.8 load_preview_smart()
    // ---------------------------
    #[wasm_bindgen(js_name = load_preview_smart)]
    pub fn load_preview_smart(
        blf_bytes: &[u8],
        dbc_texts: JsValue,
        channel_map: JsValue,
        file_size: u64,
    ) -> Result<JsValue, JsValue> {
        let slice_len: usize = if file_size <= 20 * 1024 * 1024 {
            blf_bytes.len()
        } else {
            let five_percent = (file_size as f64 * 0.05) as usize;
            std::cmp::min(five_percent, 100 * 1024 * 1024)
        };

        let slice = &blf_bytes[0..std::cmp::min(slice_len, blf_bytes.len())];
        let session = BlfSession::new(slice, dbc_texts, channel_map)?;
        let frame_count = session.frames.len();

        // Always return up to 50 frames for preview (channel-tagged signal names).
        let take_n = std::cmp::min(50usize, frame_count);
        serde_wasm_bindgen::to_value(&session.frames[0..take_n])
            .map_err(|e| JsValue::from_str(&format!("serde failed: {:?}", e)))
    }

    // ---------------------------
    // 2.9 export_csv_stream()
    // ---------------------------
    #[wasm_bindgen(js_name = export_csv_stream)]
    pub fn export_csv_stream(
        blf_bytes: &[u8],
        dbc_texts: JsValue,
        channel_map: JsValue,
        progress_cb: &Function,
    ) -> Result<Vec<u8>, JsValue> {
        // parse DBCs (same pattern as constructor)
        let dbc_texts_vec: Vec<String> = serde_wasm_bindgen::from_value(dbc_texts)
            .map_err(|e| JsValue::from_str(&format!("dbc_texts must be array of strings: {:?}", e)))?;
        let channel_map_vec: Vec<u8> = serde_wasm_bindgen::from_value(channel_map)
            .map_err(|e| JsValue::from_str(&format!("channel_map must be array of u8: {:?}", e)))?;

        let mut dbc_map: HashMap<u8, DBC> = HashMap::new();
        for (text, chan) in dbc_texts_vec.iter().zip(channel_map_vec.iter()) {
            let dbc = DBC::try_from(text.as_str())
                .map_err(|e| JsValue::from_str(&format!("Failed to parse DBC: {:?}", e)))?;
            dbc_map.insert(*chan, dbc);
        }

        // Stream-parse the full BLF (use the full buffer supplied)
        let cursor = Cursor::new(blf_bytes);
        let blf = BlfFile::from_reader(cursor)
            .map_err(|(e, _)| JsValue::from_str(&format!("Failed to parse BLF: {:?}", e)))?;

        let mut wtr = csv::WriterBuilder::new().has_headers(true).from_writer(vec![]);
        wtr.write_record(&[
            "Time [s]", "Channel", "ID", "Name", "Event Type", "Dir", "DLC", "Data"
        ]).map_err(|e| JsValue::from_str(&format!("csv write failed: {:?}", e)))?;

        let mut frame_count: usize = 0;
        for obj in blf {
            if let Some(frame) = frame_from_obj(&obj.data, &dbc_map, None) {
                frame_count += 1;
                wtr.write_record(&[
                    format!("{:.6}", frame.timestamp),
                    frame.channel,
                    format!("0x{:X}", frame.id),
                    frame.name,
                    frame.event_type,
                    frame.dir,
                    frame.dlc.to_string(),
                    frame.data.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" "),
                ]).map_err(|e| JsValue::from_str(&format!("csv write failed: {:?}", e)))?;

                // call progress callback every N frames
                if frame_count % 10_000 == 0 {
                    let _ = progress_cb.call1(&JsValue::NULL, &JsValue::from_f64(frame_count as f64));
                }
            }
        }

        wtr.into_inner()
            .map_err(|e| JsValue::from_str(&format!("CSV finalize failed: {:?}", e)))
    }

    // ---------------------------
    // 2.10 decimated_stream()
    // ---------------------------
    #[wasm_bindgen(js_name = decimated_stream)]
    pub fn decimated_stream(
        blf_bytes: &[u8],
        dbc_texts: JsValue,
        channel_map: JsValue,
        max_points: usize,
        progress_cb: &Function,
    ) -> Result<JsValue, JsValue> {
        // parse DBCs
        let dbc_texts_vec: Vec<String> = serde_wasm_bindgen::from_value(dbc_texts)
            .map_err(|e| JsValue::from_str(&format!("dbc_texts must be array of strings: {:?}", e)))?;
        let channel_map_vec: Vec<u8> = serde_wasm_bindgen::from_value(channel_map)
            .map_err(|e| JsValue::from_str(&format!("channel_map must be array of u8: {:?}", e)))?;

        let mut dbc_map: HashMap<u8, DBC> = HashMap::new();
        for (text, chan) in dbc_texts_vec.iter().zip(channel_map_vec.iter()) {
            let dbc = DBC::try_from(text.as_str())
                .map_err(|e| JsValue::from_str(&format!("Failed to parse DBC: {:?}", e)))?;
            dbc_map.insert(*chan, dbc);
        }

        // First pass: count frames of interest
        let cursor = Cursor::new(blf_bytes);
        let blf = BlfFile::from_reader(cursor)
            .map_err(|(e, _)| JsValue::from_str(&format!("Failed to parse BLF: {:?}", e)))?;
        let total_frames = blf.into_iter()
            .filter(|o| matches!(o.data, ObjectTypes::CanMessage86(_)))
            .count();

        // Second pass: decimate
        let cursor2 = Cursor::new(blf_bytes);
        let blf2 = BlfFile::from_reader(cursor2)
            .map_err(|(e, _)| JsValue::from_str(&format!("Failed to parse BLF (2): {:?}", e)))?;

        let step = std::cmp::max(1, total_frames / max_points.max(1));
        let mut times: Vec<f64> = Vec::new();
        let mut signals_map: HashMap<String, Vec<f64>> = HashMap::new();

        let mut count = 0usize;
        for obj in blf2 {
            if let Some(frame) = frame_from_obj(&obj.data, &dbc_map, None) {
                if count % step == 0 {
                    times.push(frame.timestamp);
                    for s in frame.signals {
                        signals_map.entry(s.signal).or_default().push(s.value);
                    }
                }
                count += 1;

                if count % 50_000 == 0 {
                    let _ = progress_cb.call1(&JsValue::NULL, &JsValue::from_f64(count as f64));
                }
            }
        }

        // Build a serde-serializable object and convert to JsValue
        let mut signals_json_map = serde_json::Map::new();
        for (k, v) in signals_map.into_iter() {
            let arr = serde_json::Value::Array(v.into_iter().map(|x| json!(x)).collect());
            signals_json_map.insert(k, arr);
        }

        serde_wasm_bindgen::to_value(&json!({
            "time": times,
            "signals": serde_json::Value::Object(signals_json_map)
        })).map_err(|e| JsValue::from_str(&format!("serde failed: {:?}", e)))
    }
}

// -------------------------------
// SECTION 3: Helper - decode a single signal (from can_dbc::Signal)
// -------------------------------
fn decode_signal_value(sig: &Signal, data: &[u8]) -> Option<f64> {
    // Read up to first 8 bytes into little-endian u64
    let mut buf = [0u8; 8];
    for i in 0..std::cmp::min(8, data.len()) {
        buf[i] = data[i];
    }

    let start = *sig.start_bit() as usize;
    let len = *sig.signal_size() as usize;
    if len == 0 || len > 64 || start + len > 64 {
        return None;
    }

    // raw little-endian u64 from buf
    let mut raw: u64 = 0;
    for (i, b) in buf.iter().enumerate().take(8) {
        raw |= (*b as u64) << (i * 8);
    }

    // Byte order handling
    let val_u64: u64 = if *sig.byte_order() == ByteOrder::BigEndian {
        // Motorola bit extraction (simple bit-by-bit gather)
        let mut acc: u64 = 0;
        for i in 0..len {
            let bitpos = start + i;
            let bit = (raw >> bitpos) & 1;
            acc |= bit << i;
        }
        acc
    } else {
        // Intel: straightforward mask+shift
        (raw >> start) & ((1u64 << len) - 1)
    };

    // Signed vs unsigned
    let signed_val: i64 = if *sig.value_type() == ValueType::Signed {
        let shift = 64usize - len;
        ((val_u64 << shift) as i64) >> shift
    } else {
        val_u64 as i64
    };

    Some(signed_val as f64 * *sig.factor() + *sig.offset())
}

// -------------------------------
// SECTION 4: Helper - decode one BLF object into a FrameRow (if CAN frame)
// -------------------------------
fn frame_from_obj(
    obj: &ObjectTypes,
    dbc_map: &HashMap<u8, DBC>,
    seen_signals: Option<&mut Vec<String>>,
) -> Option<FrameRow> {
    if let ObjectTypes::CanMessage86(cf) = obj {
        let ts = cf.header.timestamp_ns as f64 / 1e9;
        let channel_str = format!("CAN{}", cf.channel);
        let id = cf.id;
        let dlc = cf.dlc;
        let data_vec = cf.data.to_vec();

        let mut frame_name = String::new();
        let mut signal_rows: Vec<SignalRow> = Vec::new();

        if let Some(dbc) = dbc_map.get(&(cf.channel as u8)) {
            if let Some(msg) = dbc.messages().iter().find(|m| m.message_id().raw() == id) {
                frame_name = msg.message_name().to_string();

                for sig in msg.signals() {
                    if let Some(val) = decode_signal_value(sig, &data_vec) {
                        let sname = format!("CAN{}.{}", cf.channel, sig.name());
                        signal_rows.push(SignalRow {
                            signal: sname.clone(),
                            value: val,
                            unit: sig.unit().to_string(),
                        });
                    }
                }
            }
        }

        // ✅ update seen_signals cleanly, after building signal_rows
        if let Some(seen) = seen_signals {
            for s in &signal_rows {
                if !seen.contains(&s.signal) {
                    seen.push(s.signal.clone());
                }
            }
        }

        return Some(FrameRow {
            timestamp: ts,
            channel: channel_str,
            id,
            name: frame_name,
            event_type: "CAN Frame".to_string(),
            dir: "Rx".to_string(),
            dlc,
            data: data_vec,
            signals: signal_rows,
        });
    }
    None
}
