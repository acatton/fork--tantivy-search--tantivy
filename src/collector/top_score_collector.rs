use super::Collector;
use collector::top_collector::TopCollector;
use collector::top_collector::TopSegmentCollector;
use collector::tweak_score_top_collector::{
    ScoreTweaker, SegmentScoreTweaker, TweakedScoreTopCollector,
};
use collector::SegmentCollector;
use fastfield::{FastFieldReader, FastValue};
use schema::Field;
use DocId;
use Result;
use Score;
use SegmentLocalId;
use SegmentReader;
use {DocAddress, TantivyError};

/// The Top Score Collector keeps track of the K documents
/// sorted by their score.
///
/// The implementation is based on a `BinaryHeap`.
/// The theorical complexity for collecting the top `K` out of `n` documents
/// is `O(n log K)`.
///
/// ```rust
/// #[macro_use]
/// extern crate tantivy;
/// use tantivy::DocAddress;
/// use tantivy::schema::{Schema, TEXT};
/// use tantivy::{Index, Result};
/// use tantivy::collector::TopDocs;
/// use tantivy::query::QueryParser;
///
/// # fn main() { example().unwrap(); }
/// fn example() -> Result<()> {
///     let mut schema_builder = Schema::builder();
///     let title = schema_builder.add_text_field("title", TEXT);
///     let schema = schema_builder.build();
///     let index = Index::create_in_ram(schema);
///     {
///         let mut index_writer = index.writer_with_num_threads(1, 3_000_000)?;
///         index_writer.add_document(doc!(
///             title => "The Name of the Wind",
///         ));
///         index_writer.add_document(doc!(
///             title => "The Diary of Muadib",
///         ));
///         index_writer.add_document(doc!(
///             title => "A Dairy Cow",
///         ));
///         index_writer.add_document(doc!(
///             title => "The Diary of a Young Girl",
///         ));
///         index_writer.commit().unwrap();
///     }
///
///     let reader = index.reader()?;
///     let searcher = reader.searcher();
///
///     let query_parser = QueryParser::for_index(&index, vec![title]);
///     let query = query_parser.parse_query("diary")?;
///     let top_docs = searcher.search(&query, &TopDocs::with_limit(2))?;
///
///     assert_eq!(&top_docs[0], &(0.7261542, DocAddress(0, 1)));
///     assert_eq!(&top_docs[1], &(0.6099695, DocAddress(0, 3)));
///
///     Ok(())
/// }
/// ```
pub struct TopDocs(TopCollector<Score>);

impl TopDocs {
    /// Creates a top score collector, with a number of documents equal to "limit".
    ///
    /// # Panics
    /// The method panics if limit is 0
    pub fn with_limit(limit: usize) -> TopDocs {
        TopDocs(TopCollector::with_limit(limit))
    }

    /// Set top-K to rank documents by a given fast field.
    ///
    /// (By default, `TopDocs` collects the top-K documents sorted by
    /// the similarity score.)
    pub fn order_by_field<TFastValue>(
        self,
        field: Field,
    ) -> impl Collector<Fruit = Vec<(TFastValue, DocAddress)>>
    where
        TFastValue: FastValue + 'static,
    {
        self.tweak_score(move |segment_reader: &SegmentReader| {
            let ff_reader: FastFieldReader<u64> = segment_reader
                .fast_fields()
                .u64_lenient(field)
                .ok_or_else(|| {
                TantivyError::SchemaError("Field is not a fast field.".to_string())
            })?;
            Ok(move |doc: DocId, _score: Score| TFastValue::from_u64(ff_reader.get(doc)))
        })
    }

    pub fn tweak_score<TScore, TSegmentScoreTweaker, TScoreTweaker>(
        self,
        score_tweaker: TScoreTweaker,
    ) -> TweakedScoreTopCollector<TScoreTweaker, TScore>
    where
        TScore: Send + Sync + Clone + PartialOrd,
        TSegmentScoreTweaker: SegmentScoreTweaker<TScore> + 'static,
        TScoreTweaker: ScoreTweaker<TScore, Child = TSegmentScoreTweaker>,
    {
        TweakedScoreTopCollector::new(score_tweaker, self.0.limit())
    }
}

impl Collector for TopDocs {
    type Fruit = Vec<(Score, DocAddress)>;

    type Child = TopScoreSegmentCollector;

    fn for_segment(
        &self,
        segment_local_id: SegmentLocalId,
        reader: &SegmentReader,
    ) -> Result<Self::Child> {
        let collector = self.0.for_segment(segment_local_id, reader)?;
        Ok(TopScoreSegmentCollector(collector))
    }

    fn requires_scoring(&self) -> bool {
        true
    }

    fn merge_fruits(&self, child_fruits: Vec<Vec<(Score, DocAddress)>>) -> Result<Self::Fruit> {
        self.0.merge_fruits(child_fruits)
    }
}

/// Segment Collector associated to `TopDocs`.
pub struct TopScoreSegmentCollector(TopSegmentCollector<Score>);

impl SegmentCollector for TopScoreSegmentCollector {
    type Fruit = Vec<(Score, DocAddress)>;

    fn collect(&mut self, doc: DocId, score: Score) {
        self.0.collect(doc, score)
    }

    fn harvest(self) -> Vec<(Score, DocAddress)> {
        self.0.harvest()
    }
}

#[cfg(test)]
mod tests {
    use super::TopDocs;
    use collector::Collector;
    use query::{Query, QueryParser};
    use schema::{Field, Schema, FAST, STORED, TEXT};
    use Score;
    use {DocAddress, Index, IndexWriter, TantivyError};

    fn make_index() -> Index {
        let mut schema_builder = Schema::builder();
        let text_field = schema_builder.add_text_field("text", TEXT);
        let schema = schema_builder.build();
        let index = Index::create_in_ram(schema);
        {
            // writing the segment
            let mut index_writer = index.writer_with_num_threads(1, 3_000_000).unwrap();
            index_writer.add_document(doc!(text_field=>"Hello happy tax payer."));
            index_writer.add_document(doc!(text_field=>"Droopy says hello happy tax payer"));
            index_writer.add_document(doc!(text_field=>"I like Droopy"));
            assert!(index_writer.commit().is_ok());
        }
        index
    }

    #[test]
    fn test_top_collector_not_at_capacity() {
        let index = make_index();
        let field = index.schema().get_field("text").unwrap();
        let query_parser = QueryParser::for_index(&index, vec![field]);
        let text_query = query_parser.parse_query("droopy tax").unwrap();
        let score_docs: Vec<(Score, DocAddress)> = index
            .reader()
            .unwrap()
            .searcher()
            .search(&text_query, &TopDocs::with_limit(4))
            .unwrap();
        assert_eq!(
            score_docs,
            vec![
                (0.81221175, DocAddress(0u32, 1)),
                (0.5376842, DocAddress(0u32, 2)),
                (0.48527452, DocAddress(0, 0))
            ]
        );
    }

    #[test]
    fn test_top_collector_at_capacity() {
        let index = make_index();
        let field = index.schema().get_field("text").unwrap();
        let query_parser = QueryParser::for_index(&index, vec![field]);
        let text_query = query_parser.parse_query("droopy tax").unwrap();
        let score_docs: Vec<(Score, DocAddress)> = index
            .reader()
            .unwrap()
            .searcher()
            .search(&text_query, &TopDocs::with_limit(2))
            .unwrap();
        assert_eq!(
            score_docs,
            vec![
                (0.81221175, DocAddress(0u32, 1)),
                (0.5376842, DocAddress(0u32, 2)),
            ]
        );
    }

    #[test]
    #[should_panic]
    fn test_top_0() {
        TopDocs::with_limit(0);
    }

    const TITLE: &str = "title";
    const SIZE: &str = "size";

    #[test]
    fn test_top_field_collector_not_at_capacity() {
        let mut schema_builder = Schema::builder();
        let title = schema_builder.add_text_field(TITLE, TEXT);
        let size = schema_builder.add_u64_field(SIZE, FAST);
        let schema = schema_builder.build();
        let (index, query) = index("beer", title, schema, |index_writer| {
            index_writer.add_document(doc!(
                title => "bottle of beer",
                size => 12u64,
            ));
            index_writer.add_document(doc!(
                title => "growler of beer",
                size => 64u64,
            ));
            index_writer.add_document(doc!(
                title => "pint of beer",
                size => 16u64,
            ));
        });
        let searcher = index.reader().unwrap().searcher();

        let top_collector = TopDocs::with_limit(4).order_by_field(size);
        let top_docs: Vec<(u64, DocAddress)> = searcher.search(&query, &top_collector).unwrap();
        assert_eq!(
            top_docs,
            vec![
                (64, DocAddress(0, 1)),
                (16, DocAddress(0, 2)),
                (12, DocAddress(0, 0))
            ]
        );
    }

    #[test]
    #[should_panic]
    fn test_field_does_not_exist() {
        let mut schema_builder = Schema::builder();
        let title = schema_builder.add_text_field(TITLE, TEXT);
        let size = schema_builder.add_u64_field(SIZE, FAST);
        let schema = schema_builder.build();
        let (index, _) = index("beer", title, schema, |index_writer| {
            index_writer.add_document(doc!(
                title => "bottle of beer",
                size => 12u64,
            ));
        });
        let searcher = index.reader().unwrap().searcher();
        let top_collector = TopDocs::with_limit(4).order_by_field::<u64>(Field(2));
        let segment_reader = searcher.segment_reader(0u32);
        top_collector
            .for_segment(0, segment_reader)
            .expect("should panic");
    }

    #[test]
    fn test_field_not_fast_field() {
        let mut schema_builder = Schema::builder();
        let title = schema_builder.add_text_field(TITLE, TEXT);
        let size = schema_builder.add_u64_field(SIZE, STORED);
        let schema = schema_builder.build();
        let (index, _) = index("beer", title, schema, |index_writer| {
            index_writer.add_document(doc!(
                title => "bottle of beer",
                size => 12u64,
            ));
        });
        let searcher = index.reader().unwrap().searcher();
        let segment = searcher.segment_reader(0);
        let top_collector = TopDocs::with_limit(4).order_by_field::<u64>(size);
        assert_matches!(
            top_collector
                .for_segment(0, segment)
                .map(|_| ())
                .unwrap_err(),
            TantivyError::SchemaError(_)
        );
    }

    fn index(
        query: &str,
        query_field: Field,
        schema: Schema,
        mut doc_adder: impl FnMut(&mut IndexWriter) -> (),
    ) -> (Index, Box<Query>) {
        let index = Index::create_in_ram(schema);

        let mut index_writer = index.writer_with_num_threads(1, 3_000_000).unwrap();
        doc_adder(&mut index_writer);
        index_writer.commit().unwrap();
        let query_parser = QueryParser::for_index(&index, vec![query_field]);
        let query = query_parser.parse_query(query).unwrap();
        (index, query)
    }

}
