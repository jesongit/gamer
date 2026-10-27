//! Saved-frame replay only. Every input/service/notification operation is rejected.
#![allow(dead_code, unused_imports)]
mod extensions {
    pub mod permissions {
        include!(concat!(env!("OUT_DIR"), "/permissions.rs"));
    }
    pub mod gamer_yaml {
        pub mod resources {
            pub fn is_function_library_path(path: &str) -> bool {
                path.rsplit('/')
                    .next()
                    .is_some_and(|p| p.starts_with("_function") && p.ends_with(".yaml"))
            }
        }
    }
}
include!(concat!(env!("OUT_DIR"), "/parser-modules.rs"));
include!(concat!(env!("OUT_DIR"), "/diagnostic-modules.rs"));
mod metrics {
    pub struct Metrics;
    pub fn global() -> &'static Metrics {
        &Metrics
    }
    impl Metrics {
        pub fn record_ncc(&self, _: u64, _: bool, _: bool) {}
    }
}
use anyhow::{anyhow, ensure, Context, Result};
use image::GenericImageView;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
};
use yaml_interp::{EventSink, HostError, HostErrorKind, HostFunctions};

#[derive(Deserialize)]
struct Fixture {
    name: String,
    frame: PathBuf,
    #[serde(default)]
    expected: Option<Value>,
    #[serde(default)]
    power_only: bool,
    #[serde(default)]
    weekly_only: bool,
    #[serde(default)]
    inject: String,
    #[serde(default)]
    inject_target: String,
    #[serde(default)]
    expected_error: Option<String>,
}
#[derive(Deserialize)]
struct Config {
    engine: PathBuf,
    #[serde(default)]
    engine_arguments: Vec<String>,
    #[serde(default)]
    engine_metadata: Value,
    #[serde(default)]
    external_traineddata_sha256: Option<String>,
    tessdata: PathBuf,
    template_roots: Vec<PathBuf>,
    fixtures: Vec<Fixture>,
}
#[derive(Default)]
struct State {
    calls: Vec<Value>,
    events: Vec<Value>,
    ocr_calls: usize,
    native_calls: usize,
    templates: Map<String, Value>,
}
struct Host<'a> {
    config: &'a Config,
    fixture: &'a Fixture,
    frame: image::RgbImage,
    png: Vec<u8>,
    state: Mutex<State>,
}
fn normalized(name: &str, args: Value) -> Result<Value> {
    let schema = native_funcs::native_functions()
        .into_iter()
        .find(|f| f.name == name)
        .ok_or_else(|| anyhow!("unknown native {name}"))?;
    let mut args = if let Value::Object(fields) = args {
        fields
    } else {
        Map::from_iter(schema.params.first().map(|p| (p.name.to_string(), args)))
    };
    for p in &schema.params {
        if !args.contains_key(p.name) {
            if let Some(v) = &p.default {
                args.insert(p.name.into(), v.clone());
            } else {
                ensure!(!p.required, "missing native {name}.{}", p.name);
            }
        }
    }
    Ok(Value::Object(args))
}
impl Host<'_> {
    fn template(&self, name: &str) -> Result<PathBuf> {
        let mut found = Vec::new();
        for root in &self.config.template_roots {
            let exact = root.join(name);
            if exact.is_file() {
                return Ok(exact);
            }
            let stem = name.strip_suffix(".png").unwrap_or(name);
            for entry in fs::read_dir(root)? {
                let path = entry?.path();
                let file = path.file_name().unwrap().to_string_lossy();
                if file.starts_with(&format!("{stem}#")) && file.ends_with(".png") {
                    found.push(path);
                }
            }
        }
        ensure!(found.len() == 1, "template {name}: {} matches", found.len());
        Ok(found.remove(0))
    }
    fn region(&self, args: &Value) -> Result<Option<[u32; 4]>> {
        let Some(a) = args.get("region").and_then(Value::as_array) else {
            return Ok(None);
        };
        ensure!(a.len() == 4, "bad region");
        let mut out = [0; 4];
        for i in 0..4 {
            let v = a[i].as_f64().ok_or_else(|| anyhow!("region number"))?;
            ensure!(v.is_finite() && v >= 0.0, "bad region");
            out[i] = (v * if i % 2 == 0 {
                self.frame.width() as f64
            } else {
                self.frame.height() as f64
            })
            .floor() as u32;
        }
        ensure!(
            out[2] > 0
                && out[3] > 0
                && out[0] + out[2] <= self.frame.width()
                && out[1] + out[3] <= self.frame.height(),
            "region outside frame"
        );
        Ok(Some(out))
    }
    fn find(&self, args: &Value) -> Result<Value> {
        let name = args["template"]
            .as_str()
            .ok_or_else(|| anyhow!("missing template"))?;
        let path = self.template(name)?;
        let region = self.region(args)?.or_else(|| {
            matcher::template_region_from_name(
                path.file_name().unwrap().to_str().unwrap(),
                self.frame.width(),
                self.frame.height(),
            )
        });
        let template_png = fs::read(&path)?;
        self.state.lock().unwrap().templates.insert(
            name.into(),
            json!({"path":path,"sha256":format!("{:x}",Sha256::digest(&template_png))}),
        );
        let found = matcher::match_template(&matcher::MatchRequest {
            screen_png: self.png.clone(),
            template_png,
            threshold: Some(args["threshold"].as_f64().unwrap_or(0.8) as f32),
            region,
            color: false,
        })?;
        Ok(match found {
            Some(m) => {
                json!({"x":m.x,"y":m.y,"width":m.width,"height":m.height,"score":m.score,"template":name,"center":{"x":(m.x as f64+m.width as f64/2.0)/self.frame.width() as f64,"y":(m.y as f64+m.height as f64/2.0)/self.frame.height() as f64}})
            }
            None => Value::Null,
        })
    }
    fn ocr(&self, args: &Value, find: bool) -> Result<Value> {
        let call = {
            let mut s = self.state.lock().unwrap();
            s.ocr_calls += 1;
            s.ocr_calls
        };
        let target = match self.fixture.inject_target.as_str() {
            "" | "power" => find,
            "cost" => !find && args["mode"] == "line",
            "counter" => !find && args["mode"] == "block",
            _ => return Err(anyhow!("unknown injection target")),
        };
        if !self.fixture.inject.is_empty() && target {
            let text = match self.fixture.inject.as_str() {
                "empty" => "",
                "low_confidence" => {
                    if find {
                        "247/300"
                    } else {
                        "1"
                    }
                }
                "unstable" => {
                    if call % 2 == 0 {
                        "246/300"
                    } else {
                        "247/300"
                    }
                }
                "missing_numerator" => "/300",
                "missing_denominator" => "247/",
                "multiple_slashes" => "247/300/300",
                "zero_denominator" => "247/0",
                "zero" => {
                    if find {
                        "0/300"
                    } else {
                        "0"
                    }
                }
                _ => return Err(anyhow!("unknown injection")),
            };
            let confidence = if self.fixture.inject == "low_confidence" {
                0.2
            } else {
                0.95
            };
            return Ok(if find {
                if text.contains('/')
                    && confidence >= args["min_confidence"].as_f64().unwrap_or(0.5)
                {
                    json!({"text":text,"confidence":confidence})
                } else {
                    Value::Null
                }
            } else {
                json!({"text":text,"confidence":confidence})
            });
        }
        let [x, y, w, h] =
            self.region(args)?
                .unwrap_or([0, 0, self.frame.width(), self.frame.height()]);
        let gray = image::DynamicImage::ImageRgb8(
            image::imageops::crop_imm(&self.frame, x, y, w, h).to_image(),
        )
        .into_luma8();
        let scale = args["scale"].as_u64().unwrap_or(2) as u32;
        let scaled = image::imageops::resize(
            &gray,
            w * scale,
            h * scale,
            image::imageops::FilterType::Triangle,
        );
        let mut png = std::io::Cursor::new(Vec::new());
        scaled.write_to(&mut png, image::ImageFormat::Png)?;
        let psm = match args["mode"].as_str().unwrap_or("block") {
            "line" => 7,
            "block" => 6,
            "sparse" => 11,
            _ => return Err(anyhow!("unknown mode")),
        };
        let mut command = Command::new(&self.config.engine);
        command.args(&self.config.engine_arguments);
        command
            .args(["stdin", "stdout", "--tessdata-dir"])
            .arg(&self.config.tessdata)
            .args([
                "-l",
                args["language"].as_str().unwrap_or("eng"),
                "--psm",
                &psm.to_string(),
                "--oem",
                "1",
                "-c",
                "user_defined_dpi=300",
                "-c",
                "tessedit_create_tsv=1",
            ])
            .env("OMP_THREAD_LIMIT", "1");
        if let Some(w) = args["whitelist"].as_str() {
            command.args(["-c", &format!("tessedit_char_whitelist={w}")]);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        child.stdin.take().unwrap().write_all(png.get_ref())?;
        let output = child.wait_with_output()?;
        ensure!(
            output.status.success(),
            "Tesseract failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result = serde_json::to_value(ocr_parser::parse_tsv(
            &output.stdout,
            w,
            h,
            w * scale,
            h * scale,
        )?)?;
        if !find {
            return Ok(result);
        }
        let needle = args["text"].as_str().unwrap();
        let confidence = args["min_confidence"].as_f64().unwrap_or(0.5);
        for value in result["lines"]
            .as_array()
            .unwrap()
            .iter()
            .chain(result["words"].as_array().unwrap().iter())
        {
            let text = value["text"].as_str().unwrap();
            let matches = if args["match"] == "exact" {
                text == needle
            } else {
                text.contains(needle)
            };
            if matches && value["confidence"].as_f64().unwrap() >= confidence {
                return Ok(value.clone());
            }
        }
        Ok(Value::Null)
    }
    fn invoke_checked(&self, name: &str, args: Value) -> Result<Value> {
        let args = normalized(name, args)?;
        {
            let mut s = self.state.lock().unwrap();
            s.native_calls += 1;
            ensure!(s.native_calls <= 5000, "probe native budget");
        }
        let result = match name {
            "find" | "wait_find" => self.find(&args),
            "find_any" => {
                let mut result = Value::Null;
                for (i, t) in args["templates"].as_array().unwrap().iter().enumerate() {
                    let mut a = args.clone();
                    a["template"] = t.clone();
                    let mut found = self.find(&a)?;
                    if !found.is_null() {
                        found["index"] = json!(i);
                        result = found;
                        break;
                    }
                }
                Ok(result)
            }
            "ocr" => self.ocr(&args, false),
            "find_text" => self.ocr(&args, true),
            "number" => numeric::number(
                args["text"].as_str().unwrap(),
                args["index"].as_u64().unwrap_or(0),
                args["format"].as_str().unwrap_or("tokens"),
            ),
            "calculate" => numeric::calculate(
                args["op"].as_str().unwrap(),
                args["a"].as_f64().unwrap(),
                args["b"].as_f64().unwrap(),
            ),
            "eq" | "ne" => {
                let eq = match (args["a"].as_f64(), args["b"].as_f64()) {
                    (Some(a), Some(b)) => a == b,
                    _ => args["a"] == args["b"],
                };
                Ok(json!(if name == "eq" { eq } else { !eq }))
            }
            "lt" | "le" | "gt" | "ge" => {
                let a = args["a"].as_f64().ok_or_else(|| anyhow!("comparison a"))?;
                let b = args["b"].as_f64().ok_or_else(|| anyhow!("comparison b"))?;
                Ok(json!(match name {
                    "lt" => a < b,
                    "le" => a <= b,
                    "gt" => a > b,
                    _ => a >= b,
                }))
            }
            "sleep" | "log" => Ok(Value::Null),
            "fail" => Err(anyhow!(
                "SCRIPT_FAIL:{}",
                args["code"].as_str().unwrap_or("")
            )),
            _ => Err(anyhow!("SIDE_EFFECT_OR_UNSUPPORTED:{name}")),
        };
        self.state.lock().unwrap().calls.push(json!({"function":name,"args":args,"result":result.as_ref().ok(),"error":result.as_ref().err().map(ToString::to_string)}));
        result
    }
}
impl HostFunctions for Host<'_> {
    fn invoke(&self, name: &str, args: Value) -> std::result::Result<Value, HostError> {
        self.invoke_checked(name, args)
            .map_err(|e| HostError::new(HostErrorKind::Failed, e.to_string()))
    }
}
impl EventSink for Host<'_> {
    fn emit(&self, event: Value) {
        self.state.lock().unwrap().events.push(event);
    }
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let option = |name: &str| {
        args.iter()
            .position(|v| v == name)
            .and_then(|i| args.get(i + 1))
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("missing {name}"))
    };
    let candidate = option("--candidate")?;
    let fixtures = option("--fixtures")?;
    let report = option("--report")?;
    let hashes: Value = serde_json::from_str(include_str!(concat!(
        env!("OUT_DIR"),
        "/source-hashes.json"
    )))?;
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap()
        .to_path_buf();
    for entry in hashes.as_array().unwrap() {
        ensure!(
            format!(
                "{:x}",
                Sha256::digest(fs::read(repo.join(entry["path"].as_str().unwrap()))?)
            ) == entry["sha256"].as_str().unwrap(),
            "source changed; rebuild probe"
        );
    }
    let source = fs::read_to_string(&candidate)?;
    let source_hash = format!("{:x}", Sha256::digest(source.as_bytes()));
    let script = syntax::parse_script(&source).map_err(|e| anyhow!("candidate syntax: {e:?}"))?;
    let config: Config = serde_json::from_slice(&fs::read(fixtures)?)?;
    let mut results = Vec::new();
    for fixture in &config.fixtures {
        let mut script = script.clone();
        let wrapper = if fixture.weekly_only {
            "run: [{read_weekly_attempts: {}, as: weekly}, {return: $weekly}]"
        } else if fixture.power_only {
            "run: [{read_dungeon_power: {}, as: power}, {return: $power}]"
        } else {
            "run: [{read_dungeon_power: {}, as: power}, {read_stamina_cost: {}, as: cost}, {read_challenge_counter: {kind: current}, as: current}, {read_challenge_counter: {kind: max}, as: maximum}, {return: {power: $power, cost: $cost, current: $current, maximum: $maximum}}]"
        };
        script.run = syntax::parse_script(wrapper)
            .map_err(|e| anyhow!("wrapper: {e:?}"))?
            .run;
        let program: yaml_interp::Program = serde_json::from_value(syntax::build_program(
            &script,
            &vec![],
            Default::default(),
            0,
        ))?;
        let png = fs::read(&fixture.frame)?;
        let frame = image::load_from_memory(&png)?.to_rgb8();
        let host = Host {
            config: &config,
            fixture,
            frame,
            png,
            state: Mutex::new(State::default()),
        };
        let result = yaml_interp::run(&program, &host, Some(&host));
        let state = host.state.lock().unwrap();
        let passed = if let Some(error) = &fixture.expected_error {
            result.as_ref().err().is_some_and(|e| e.contains(error))
        } else {
            result
                .as_ref()
                .is_ok_and(|v| Some(v) == fixture.expected.as_ref())
        };
        let item = json!({"name":fixture.name,"frame":fixture.frame,"frame_sha256":format!("{:x}",Sha256::digest(&host.png)),"passed":passed,"expected":fixture.expected,"expected_error":fixture.expected_error,"result":result.as_ref().ok(),"error":result.as_ref().err(),"ocr_calls":state.ocr_calls,"templates":state.templates,"calls":state.calls});
        println!(
            "{}",
            json!({"fixture":fixture.name,"passed":passed,"result":item["result"],"error":item["error"]})
        );
        results.push(item);
    }
    let passed = results.iter().all(|v| v["passed"] == true);
    let engine_version = Command::new(&config.engine)
        .args(&config.engine_arguments)
        .arg("--version")
        .output()?;
    ensure!(
        engine_version.status.success(),
        "cannot identify OCR engine"
    );
    let traineddata_hash = if let Some(hash) = &config.external_traineddata_sha256 {
        ensure!(
            hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid external traineddata hash"
        );
        hash.clone()
    } else {
        format!(
            "{:x}",
            Sha256::digest(fs::read(config.tessdata.join("eng.traineddata"))?)
        )
    };
    let output = json!({"passed":passed,"scope":"Current authoritative YAML parser/interpreter, Core NCC/numeric/TSV parser, saved real frames and configured Tesseract. Input/service/notification calls rejected. Polling, OCR queue/transport and live-game behavior are not simulated.","candidate_sha256":source_hash,"source_hashes":hashes,"engine":{"path":config.engine,"launcher_sha256":format!("{:x}",Sha256::digest(fs::read(&config.engine)?)),"arguments":config.engine_arguments,"metadata":config.engine_metadata,"version":String::from_utf8_lossy(&engine_version.stdout),"eng_traineddata_sha256":traineddata_hash},"device_inputs":0,"production_accessed":false,"cases":results});
    fs::create_dir_all(report.parent().unwrap())?;
    fs::write(report, serde_json::to_vec_pretty(&output)?)?;
    ensure!(passed, "compatibility cases failed");
    Ok(())
}
