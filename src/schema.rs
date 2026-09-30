//! The index schema, shared by the indexer and the query side.

use tantivy::schema::{
    Field, IndexRecordOption, NumericOptions, Schema, TextFieldIndexing, TextOptions, FAST, INDEXED,
};

use crate::tokenizer;

/// Version of the on disk format. The index is recreated when it differs, see
/// [`crate::engine`]. Bump it with every change of the schema, the analyzers
/// or what a document means.
pub const FORMAT_VERSION: u32 = 2;

/// Handles to every field of the log index.
#[derive(Clone, Debug)]
pub struct Fields {
    pub ts: Field,
    pub ip: Field,
    /// The address as a number (IPv4 is mapped into IPv6), for range queries.
    pub ip_addr: Field,
    pub status: Field,
    pub method: Field,
    pub browser: Field,
    pub os: Field,
    pub device_type: Field,
    pub region_code: Field,
    pub province: Field,
    pub city: Field,
    pub c1: Field,
    pub c2: Field,
    pub c3: Field,
    pub c4: Field,
    /// Request path: phrase searchable, the fast column keeps the whole value.
    pub path: Field,
    pub referer: Field,
    /// User agent: phrase searchable, the fast column keeps the whole value.
    pub user_agent: Field,
    pub bytes_sent: Field,
    pub request_time: Field,
    pub upstream_time: Field,
    /// The original line, stored and searchable from the search box.
    pub raw: Field,
    pub main_log_path: Field,
    /// First line fingerprint of the content the line comes from.
    pub fp: Field,
    /// Byte offset of the line in the decompressed content.
    pub off: Field,
}

/// The custom fields of the geo database, in order.
pub const CUSTOM_FIELDS: [&str; 4] = ["c1", "c2", "c3", "c4"];

/// Untokenized text that is indexed and kept as a fast column.
fn keyword_fast() -> TextOptions {
    TextOptions::default()
        .set_indexing_options(
            TextFieldIndexing::default().set_tokenizer("raw").set_index_option(IndexRecordOption::Basic),
        )
        .set_fast(Some("raw"))
}

/// Untokenized text that is indexed only.
fn keyword() -> TextOptions {
    TextOptions::default().set_indexing_options(
        TextFieldIndexing::default().set_tokenizer("raw").set_index_option(IndexRecordOption::Basic),
    )
}

/// Tokenized text with positions for phrase queries, plus a fast column with
/// the untokenized value when `fast` is set.
fn phrase(fast: bool) -> TextOptions {
    let options = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer(tokenizer::PHRASE_NAME)
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    );
    if fast {
        options.set_fast(Some("raw"))
    } else {
        options
    }
}

/// The stored line, tokenized for the search box. Positions make quoted
/// phrases possible and cost index size and time, so they are optional.
fn raw_text(positions: bool) -> TextOptions {
    let record = if positions { IndexRecordOption::WithFreqsAndPositions } else { IndexRecordOption::Basic };
    TextOptions::default()
        .set_stored()
        .set_indexing_options(TextFieldIndexing::default().set_tokenizer(tokenizer::TEXT_NAME).set_index_option(record))
}

/// Builds the schema. `positions` indexes the positions of the raw line.
pub fn build(positions: bool) -> (Schema, Fields) {
    let mut b = Schema::builder();
    let fields = Fields {
        ts: b.add_i64_field("ts", INDEXED | FAST),
        ip: b.add_text_field("ip", keyword_fast()),
        ip_addr: b.add_ip_addr_field("ip_addr", INDEXED | FAST),
        status: b.add_u64_field("status", INDEXED | FAST),
        method: b.add_text_field("method", keyword_fast()),
        browser: b.add_text_field("browser", keyword_fast()),
        os: b.add_text_field("os", keyword_fast()),
        device_type: b.add_text_field("device_type", keyword_fast()),
        region_code: b.add_text_field("region_code", keyword_fast()),
        province: b.add_text_field("province", keyword_fast()),
        city: b.add_text_field("city", keyword_fast()),
        c1: b.add_text_field("c1", keyword_fast()),
        c2: b.add_text_field("c2", keyword_fast()),
        c3: b.add_text_field("c3", keyword_fast()),
        c4: b.add_text_field("c4", keyword_fast()),
        path: b.add_text_field("path", phrase(true)),
        referer: b.add_text_field("referer", phrase(false)),
        user_agent: b.add_text_field("user_agent", phrase(true)),
        bytes_sent: b.add_u64_field("bytes_sent", NumericOptions::default().set_fast()),
        request_time: b.add_f64_field("request_time", NumericOptions::default().set_fast()),
        upstream_time: b.add_f64_field("upstream_time", NumericOptions::default().set_fast()),
        raw: b.add_text_field("raw", raw_text(positions)),
        main_log_path: b.add_text_field("main_log_path", keyword()),
        fp: b.add_text_field("fp", keyword()),
        off: b.add_u64_field("off", NumericOptions::default().set_fast()),
    };
    (b.build(), fields)
}

impl Fields {
    /// The four custom geo fields.
    pub fn custom(&self) -> [Field; 4] {
        [self.c1, self.c2, self.c3, self.c4]
    }
}
