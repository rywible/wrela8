use std::collections::BTreeMap;
use std::sync::OnceLock;

const CENSUS_TOML: &str = include_str!("../../../tests/census.toml");

#[derive(Debug)]
pub struct CensusFile {
    pub cost_dimension_ids: Vec<u32>,
    pub cost_dimension_names: Vec<String>,
    pub internal_error: InternalErrorCensus,
    pub emitted_a64: EmittedA64Census,
    pub placed_static_fixed_core_names: Vec<String>,
    pub guest_fn_key: GuestFnKeyCensus,
    pub corpus_sema_pins: Vec<CorpusSemaPin>,
}

#[derive(Debug)]
pub struct InternalErrorCensus {
    pub total: usize,
    pub sites_by_file: BTreeMap<String, usize>,
}

#[derive(Debug, Clone)]
pub struct EmitterEntry {
    pub name: String,
    pub file: String,
    pub words: usize,
    pub category: EmittedCategory,
    pub note: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum EmittedCategory {
    Floor,
    ImageStatic,
    NotYetMigrated,
    Unclassified,
}

#[derive(Debug)]
pub struct EmittedA64Census {
    pub encode_enc_site_total: usize,
    pub floor_sum_of_rows: usize,
    pub floor_words: usize,
    pub image_static_sum_of_rows: usize,
    pub not_yet_migrated_sum_of_rows: usize,
    pub unclassified_sum_of_rows: usize,
    pub grand_total_sum_of_rows: usize,
    pub encode_enc_sites_by_file: BTreeMap<String, usize>,
    pub backend_emitters: Vec<String>,
    pub non_inventory: Vec<String>,
    pub wrapper_emitters: Vec<String>,
    pub entries: Vec<EmitterEntry>,
}

#[derive(Debug)]
pub struct GuestFnKeyCensus {
    pub zero_fn_dumps: Vec<(String, String)>,
    pub required: Vec<(String, Vec<String>)>,
}

#[derive(Debug, Clone)]
pub struct CorpusSemaPin {
    pub key: String,
    pub loc: String,
    pub kind: String,
    pub why: Option<String>,
}

pub fn data() -> &'static CensusFile {
    static DATA: OnceLock<CensusFile> = OnceLock::new();
    DATA.get_or_init(parse_census)
}

fn parse_census() -> CensusFile {
    let value: toml::Value = CENSUS_TOML
        .parse()
        .unwrap_or_else(|e| panic!("tests/census.toml: parse failed: {e}"));
    let cost_dimension = value
        .get("cost_dimension")
        .expect("census.toml: [cost_dimension]");
    let cost_dimension_ids: Vec<u32> = cost_dimension
        .get("ids")
        .and_then(|v| v.as_array())
        .expect("census.toml: cost_dimension.ids")
        .iter()
        .map(|v| {
            v.as_integer()
                .and_then(|n| u32::try_from(n).ok())
                .expect("census.toml: cost_dimension.ids must contain u32 values")
        })
        .collect();
    let cost_dimension_names = parse_string_array(
        cost_dimension
            .get("names")
            .expect("census.toml: cost_dimension.names"),
    );
    assert_eq!(
        cost_dimension_ids.len(),
        cost_dimension_names.len(),
        "census.toml: cost_dimension.ids/names length mismatch"
    );
    CensusFile {
        cost_dimension_ids,
        cost_dimension_names,
        internal_error: parse_internal_error(&value),
        emitted_a64: parse_emitted_a64(&value),
        placed_static_fixed_core_names: parse_string_array(
            value
                .get("placed_static")
                .and_then(|t| t.get("fixed_core_names"))
                .expect("census.toml: placed_static.fixed_core_names"),
        ),
        guest_fn_key: parse_guest_fn_key(&value),
        corpus_sema_pins: parse_corpus_sema(&value),
    }
}

fn parse_internal_error(value: &toml::Value) -> InternalErrorCensus {
    let t = value
        .get("internal_error")
        .expect("census.toml: [internal_error]");
    let total = t
        .get("total")
        .and_then(|v| v.as_integer())
        .expect("census.toml: internal_error.total") as usize;
    let sites = t
        .get("sites_by_file")
        .and_then(|v| v.as_table())
        .expect("census.toml: internal_error.sites_by_file");
    let mut sites_by_file = BTreeMap::new();
    for (k, v) in sites {
        let n = v
            .as_integer()
            .unwrap_or_else(|| panic!("census.toml: internal_error.sites_by_file[{k}]"))
            as usize;
        sites_by_file.insert(k.clone(), n);
    }
    InternalErrorCensus {
        total,
        sites_by_file,
    }
}

fn parse_emitted_a64(value: &toml::Value) -> EmittedA64Census {
    let t = value
        .get("emitted_a64")
        .expect("census.toml: [emitted_a64]");
    let usize_field = |name: &str| -> usize {
        t.get(name)
            .and_then(|v| v.as_integer())
            .unwrap_or_else(|| panic!("census.toml: emitted_a64.{name}")) as usize
    };
    let sites = t
        .get("encode_enc_sites_by_file")
        .and_then(|v| v.as_table())
        .expect("census.toml: emitted_a64.encode_enc_sites_by_file");
    let mut encode_enc_sites_by_file = BTreeMap::new();
    for (k, v) in sites {
        let n = v
            .as_integer()
            .unwrap_or_else(|| panic!("census.toml: encode_enc_sites_by_file[{k}]"))
            as usize;
        encode_enc_sites_by_file.insert(k.clone(), n);
    }
    let entries_val = t
        .get("entries")
        .and_then(|v| v.as_array())
        .expect("census.toml: emitted_a64.entries");
    let mut entries = Vec::with_capacity(entries_val.len());
    for e in entries_val {
        let name = e
            .get("name")
            .and_then(|v| v.as_str())
            .expect("entry.name")
            .to_string();
        let file = e
            .get("file")
            .and_then(|v| v.as_str())
            .expect("entry.file")
            .to_string();
        let words = e
            .get("words")
            .and_then(|v| v.as_integer())
            .expect("entry.words") as usize;
        let category = match e
            .get("category")
            .and_then(|v| v.as_str())
            .expect("entry.category")
        {
            "floor" => EmittedCategory::Floor,
            "image_static" => EmittedCategory::ImageStatic,
            "not_yet_migrated" => EmittedCategory::NotYetMigrated,
            "unclassified" => EmittedCategory::Unclassified,
            other => panic!("census.toml: unknown emitted_a64 category `{other}`"),
        };
        let note = e
            .get("note")
            .and_then(|v| v.as_str())
            .expect("entry.note")
            .to_string();
        entries.push(EmitterEntry {
            name,
            file,
            words,
            category,
            note,
        });
    }
    EmittedA64Census {
        encode_enc_site_total: usize_field("encode_enc_site_total"),
        floor_sum_of_rows: usize_field("floor_sum_of_rows"),
        floor_words: usize_field("floor_words"),
        image_static_sum_of_rows: usize_field("image_static_sum_of_rows"),
        not_yet_migrated_sum_of_rows: usize_field("not_yet_migrated_sum_of_rows"),
        unclassified_sum_of_rows: usize_field("unclassified_sum_of_rows"),
        grand_total_sum_of_rows: usize_field("grand_total_sum_of_rows"),
        encode_enc_sites_by_file,
        backend_emitters: parse_string_array(
            t.get("backend_emitters")
                .expect("census.toml: emitted_a64.backend_emitters"),
        ),
        non_inventory: parse_string_array(
            t.get("non_inventory")
                .expect("census.toml: emitted_a64.non_inventory"),
        ),
        wrapper_emitters: parse_string_array(
            t.get("wrapper_emitters")
                .expect("census.toml: emitted_a64.wrapper_emitters"),
        ),
        entries,
    }
}

fn parse_guest_fn_key(value: &toml::Value) -> GuestFnKeyCensus {
    let t = value
        .get("guest_fn_key")
        .expect("census.toml: [guest_fn_key]");
    let zero = t
        .get("zero_fn_dumps")
        .and_then(|v| v.as_array())
        .expect("census.toml: guest_fn_key.zero_fn_dumps");
    let mut zero_fn_dumps = Vec::with_capacity(zero.len());
    for e in zero {
        zero_fn_dumps.push((
            e.get("path")
                .and_then(|v| v.as_str())
                .expect("zero.path")
                .to_string(),
            e.get("reason")
                .and_then(|v| v.as_str())
                .expect("zero.reason")
                .to_string(),
        ));
    }
    let req = t
        .get("required")
        .and_then(|v| v.as_array())
        .expect("census.toml: guest_fn_key.required");
    let mut required = Vec::with_capacity(req.len());
    for e in req {
        let path = e
            .get("path")
            .and_then(|v| v.as_str())
            .expect("required.path")
            .to_string();
        let keys = parse_string_array(e.get("keys").expect("required.keys"));
        required.push((path, keys));
    }
    GuestFnKeyCensus {
        zero_fn_dumps,
        required,
    }
}

fn parse_corpus_sema(value: &toml::Value) -> Vec<CorpusSemaPin> {
    let t = value
        .get("corpus_sema")
        .expect("census.toml: [corpus_sema]");
    let pins = t
        .get("pins")
        .and_then(|v| v.as_array())
        .expect("census.toml: corpus_sema.pins");
    let mut out = Vec::with_capacity(pins.len());
    for e in pins {
        out.push(CorpusSemaPin {
            key: e
                .get("key")
                .and_then(|v| v.as_str())
                .expect("pin.key")
                .to_string(),
            loc: e
                .get("loc")
                .and_then(|v| v.as_str())
                .expect("pin.loc")
                .to_string(),
            kind: e
                .get("kind")
                .and_then(|v| v.as_str())
                .expect("pin.kind")
                .to_string(),
            why: e.get("why").and_then(|v| v.as_str()).map(|s| s.to_string()),
        });
    }
    out
}

fn parse_string_array(v: &toml::Value) -> Vec<String> {
    v.as_array()
        .unwrap_or_else(|| panic!("census.toml: expected string array, got {v}"))
        .iter()
        .map(|x| {
            x.as_str()
                .unwrap_or_else(|| panic!("census.toml: expected string in array, got {x}"))
                .to_string()
        })
        .collect()
}
