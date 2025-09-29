use wasm_bindgen::prelude::*;
use serde::Serialize;
use serde_wasm_bindgen;
use std::collections::HashMap;
use std::io::Cursor;

use ablf::{BlfFile, ObjectTypes};
use can_dbc::{DBC, Signal, ByteOrder, ValueType};

// -------------------------------
// Data structures returned to JS
// -------------------------------
#[derive(Serialize, Debug, Clone)]
pub struct SignalRow {
    pub signal: String,
    pub value: f64,
    pub unit: String,
}

#[derive(Serialize, Debug, Clone)]
pub struct FrameRow {
    pub timestamp: f64,
    pub channel: String,
    pub id: u32,
    pub name: String,
    pub event_type: String,
    pub dir: String, // Rx/Tx
    pub dlc: u8,
    pub data: Vec<u8>,
    pub signals: Vec<SignalRow>,
}

// -------------------------------
// BlfSession (WASM-visible)
// -------------------------------
#[wasm_bindgen]
pub struct BlfSession {
    frames: Vec<FrameRow>,
    signal_names: Vec<String>,
}

#[wasm_bindgen]
impl BlfSession {
    #[wasm_bindgen(constructor)]
    pub fn new(
        blf_bytes: &[u8],
        dbc_texts: JsValue,
        channel_map: JsValue,
    ) -> Result<BlfSession, JsValue> {
        let dbc_texts_vec: Vec<String> = serde_wasm_bindgen::from_value(dbc_texts)
            .map_err(|e| JsValue::from_str(&format!("dbc_texts must be array of strings: {:?}", e)))?;
        let channel_map_vec: Vec<u8> = serde_wasm_bindgen::from_value(channel_map)
            .map_err(|e| JsValue::from_str(&format!("channel_map must be array of u8: {:?}", e)))?;

        if dbc_texts_vec.len() != channel_map_vec.len() {
            return Err(JsValue::from_str(
                "dbc_texts and channel_map must have same length",
            ));
        }

        let mut dbc_map: HashMap<u8, DBC> = HashMap::new();
        for (text, chan) in dbc_texts_vec.iter().zip(channel_map_vec.iter()) {
            let dbc = DBC::try_from(text.as_str())
                .map_err(|e| JsValue::from_str(&format!("Failed to parse DBC for channel {}: {:?}", chan, e)))?;
            dbc_map.insert(*chan, dbc);
        }

        let cursor = Cursor::new(blf_bytes);
        let blf = BlfFile::from_reader(cursor)
            .map_err(|(e, _)| JsValue::from_str(&format!("Failed to parse BLF: {:?}", e)))?;

        let mut frames: Vec<FrameRow> = Vec::new();
        let mut seen_signals: Vec<String> = Vec::new();

        for obj in blf {
            if let ObjectTypes::CanMessage86(cf) = obj.data {
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
                                let sname = sig.name().to_string();
                                signal_rows.push(SignalRow {
                                    signal: sname.clone(),
                                    value: val,
                                    unit: sig.unit().to_string(),
                                });
                                if !seen_signals.contains(&sname) {
                                    seen_signals.push(sname);
                                }
                            }
                        }
                    }
                }

                frames.push(FrameRow {
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
        }

        seen_signals.sort();
        Ok(BlfSession { frames, signal_names: seen_signals })
    }

    pub fn stats(&self) -> Result<JsValue, JsValue> {
        let count = self.frames.len() as u32;
        let (first, last) = if !self.frames.is_empty() {
            (
                self.frames.first().unwrap().timestamp,
                self.frames.last().unwrap().timestamp,
            )
        } else {
            (0.0f64, 0.0f64)
        };
        let sig_count = self.signal_names.len() as u32;
        serde_wasm_bindgen::to_value(&(count, first, last, sig_count))
            .map_err(|e| JsValue::from_str(&format!("serde failed: {:?}", e)))
    }

    pub fn preview(&self, n: usize) -> Result<JsValue, JsValue> {
        let take_n = std::cmp::min(n, self.frames.len());
        serde_wasm_bindgen::to_value(&self.frames[0..take_n])
            .map_err(|e| JsValue::from_str(&format!("serde failed: {:?}", e)))
    }

    pub fn signals(&self) -> Result<JsValue, JsValue> {
        serde_wasm_bindgen::to_value(&self.signal_names)
            .map_err(|e| JsValue::from_str(&format!("serde failed: {:?}", e)))
    }

    pub fn decimated(
        &self,
        max_points: usize,
        keep_signals: JsValue,
    ) -> Result<JsValue, JsValue> {
        let total = self.frames.len();
        if total == 0 {
            return serde_wasm_bindgen::to_value(
                &serde_json::json!({"time": [], "signals": {}})
            ).map_err(|e| JsValue::from_str(&format!("serde failed: {:?}", e)));
        }

        let keep_opt: Option<Vec<String>> =
            if keep_signals.is_null() || keep_signals.is_undefined() {
                None
            } else {
                Some(serde_wasm_bindgen::from_value(keep_signals)
                    .map_err(|e| JsValue::from_str(&format!("keep_signals must be array of strings: {:?}", e)))?)
            };

        let keys: Vec<String> = if let Some(ref k) = keep_opt {
            k.clone()
        } else {
            self.signal_names.clone()
        };

        let step = std::cmp::max(1, total / max_points);
        let mut dec_time = Vec::with_capacity((total + step - 1) / step);
        let mut dec_signals: HashMap<String, Vec<Option<f64>>> = HashMap::new();
        let mut last_seen: HashMap<String, Option<f64>> = HashMap::new();

        for k in &keys {
            dec_signals.insert(k.clone(), Vec::new());
            last_seen.insert(k.clone(), None);
        }

        for (i, frame) in self.frames.iter().enumerate() {
            for s in &frame.signals {
                last_seen.insert(s.signal.clone(), Some(s.value));
            }

            if i % step == 0 {
                dec_time.push(frame.timestamp);
                for k in &keys {
                    let arr = dec_signals.get_mut(k).unwrap();
                    arr.push(last_seen.get(k).cloned().unwrap_or(None));
                }
            }
        }

        let mut out_signals = serde_json::Map::new();
        for (k, vec_opt) in dec_signals.into_iter() {
            let arr_values: Vec<serde_json::Value> = vec_opt
                .into_iter()
                .map(|o| o.map_or(serde_json::Value::Null, |v| serde_json::json!(v)))
                .collect();
            out_signals.insert(k, serde_json::Value::Array(arr_values));
        }

        serde_wasm_bindgen::to_value(&serde_json::json!({
            "time": dec_time,
            "signals": serde_json::Value::Object(out_signals)
        })).map_err(|e| JsValue::from_str(&format!("serde failed: {:?}", e)))
    }

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
            let mut row: Vec<String> = Vec::new();
            row.push(format!("{:.6}", f.timestamp));
            row.push(f.channel.clone());
            row.push(format!("0x{:X}", f.id));
            row.push(f.name.clone());
            row.push(f.event_type.clone());
            row.push(f.dir.clone());
            row.push(f.dlc.to_string());
            row.push(f.data.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" "));

            let mut sig_map: HashMap<&str, f64> = HashMap::new();
            for s in &f.signals { sig_map.insert(&s.signal, s.value); }

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

    pub fn free_memory(&mut self) {
        self.frames.clear();
        self.signal_names.clear();
    }
}

// -------------------------------
// Helper: decode value for a single signal
// -------------------------------
fn decode_signal_value(sig: &Signal, data: &[u8]) -> Option<f64> {
    let mut buf = [0u8; 8];
    for i in 0..std::cmp::min(8, data.len()) {
        buf[i] = data[i];
    }

    let start = *sig.start_bit() as usize;
    let len = *sig.signal_size() as usize;
    if len == 0 || len > 64 || start + len > 64 {
        return None;
    }

    let mut raw: u64 = 0;
    for (i, b) in buf.iter().enumerate().take(8) {
        raw |= (*b as u64) << (i * 8);
    }

    let val_u64: u64 = if *sig.byte_order() == ByteOrder::BigEndian {
        let mut acc: u64 = 0;
        for i in 0..len {
            let bitpos = start + i;
            let bit = (raw >> bitpos) & 1;
            acc |= bit << i;
        }
        acc
    } else {
        (raw >> start) & ((1u64 << len) - 1)
    };

    let signed_val: i64 = if *sig.value_type() == ValueType::Signed {
        let shift = 64usize - len;
        ((val_u64 << shift) as i64) >> shift
    } else {
        val_u64 as i64
    };

    Some(signed_val as f64 * *sig.factor() + *sig.offset())
}
