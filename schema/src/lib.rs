//! This module contains the schema definition for IOx
use std::{
    cmp::Ordering,
    collections::HashMap,
    convert::{TryFrom, TryInto},
    fmt,
    mem::{size_of, size_of_val},
    sync::Arc,
};

use arrow::datatypes::{
    DataType as ArrowDataType, Field as ArrowField, Schema as ArrowSchema,
    SchemaRef as ArrowSchemaRef, TimeUnit,
};
use hashbrown::HashSet;

use crate::sort::SortKey;
use selection::Selection;
use snafu::{OptionExt, Snafu};

/// The name of the timestamp column in the InfluxDB datamodel
pub const TIME_COLUMN_NAME: &str = "time";

/// The Timezone to use for InfluxDB timezone (should be a constant)
#[allow(non_snake_case)]
pub fn TIME_DATA_TIMEZONE() -> Option<String> {
    // TODO: we should use the "UTC" timezone as that is what the
    // InfluxDB data model timestamps are relative to. However,
    // DataFusion doesn't currently do a great job with such
    // timezones so punting for now
    //Some(String::from("UTC"));
    None
}

/// the [`ArrowDataType`] to use for InfluxDB timestamps
#[allow(non_snake_case)]
pub fn TIME_DATA_TYPE() -> ArrowDataType {
    ArrowDataType::Timestamp(TimeUnit::Nanosecond, TIME_DATA_TIMEZONE())
}

pub mod builder;
pub mod merge;
pub mod selection;
pub mod sort;

pub use builder::SchemaBuilder;

/// Database schema creation / validation errors.
#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Error: Duplicate column name found in schema: '{}'", column_name,))]
    DuplicateColumnName { column_name: String },

    #[snafu(display(
    "Error: Incompatible metadata type found in schema for column '{}'. Metadata specified {:?} which is incompatible with actual type {:?}",
    column_name, influxdb_column_type, actual_type
    ))]
    IncompatibleMetadata {
        column_name: String,
        influxdb_column_type: InfluxColumnType,
        actual_type: ArrowDataType,
    },

    #[snafu(display("Column not found '{}'", column_name))]
    ColumnNotFound { column_name: String },

    #[snafu(display("Sort column not found '{}'", column_name))]
    SortColumnNotFound { column_name: String },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Schema for an IOx table.
///
/// This struct is a wrapper around an Arrow `SchemaRef` that knows
/// how to create and interpret the "user defined metadata" added to that schema
/// by IOx.
///
/// The metadata can be used to map back and forth to the InfluxDB
/// data model, which is described in the
/// [documentation](https://docs.influxdata.com/influxdb/v2.0/reference/syntax/line-protocol/).
///
/// Specifically, each column in the Arrow schema has a corresponding
/// InfluxDB data model type of Tag, Field or Timestamp which is stored in
/// the metadata field of the ArrowSchemaRef
#[derive(Debug, Clone, PartialEq)]
pub struct Schema {
    /// All the actual data lives on the metadata structure in
    /// `ArrowSchemaRef` and this structure knows how to access that
    /// metadata
    inner: ArrowSchemaRef,
}

impl From<Schema> for ArrowSchemaRef {
    fn from(s: Schema) -> Self {
        s.inner
    }
}

impl From<&Schema> for ArrowSchemaRef {
    fn from(s: &Schema) -> Self {
        s.as_arrow()
    }
}

impl TryFrom<ArrowSchemaRef> for Schema {
    type Error = Error;

    fn try_from(value: ArrowSchemaRef) -> Result<Self, Self::Error> {
        Self::try_from_arrow(value)
    }
}

const MEASUREMENT_METADATA_KEY: &str = "iox::measurement::name";
const COLUMN_METADATA_KEY: &str = "iox::column::type";

impl Schema {
    /// Create a new Schema wrapper over the schema
    ///
    /// All metadata validation is done on creation (todo maybe offer
    /// a fallible version where the checks are done on access)?
    fn try_from_arrow(inner: ArrowSchemaRef) -> Result<Self> {
        // Validate fields
        {
            // All column names must be unique
            let mut field_names = HashSet::with_capacity(inner.fields().len());

            for field in inner.fields() {
                let column_name = field.name();
                if !field_names.insert(column_name.as_str()) {
                    return Err(Error::DuplicateColumnName {
                        column_name: column_name.to_string(),
                    });
                }

                // for each field, ensure any type specified by the metadata
                // is compatible with the actual type of the field
                if let Some(influxdb_column_type) = get_influx_type(field) {
                    let actual_type = field.data_type();
                    if !influxdb_column_type.valid_arrow_type(actual_type) {
                        return Err(Error::IncompatibleMetadata {
                            column_name: column_name.to_string(),
                            influxdb_column_type,
                            actual_type: actual_type.clone(),
                        });
                    }
                }
            }
        }

        Ok(Self { inner })
    }

    /// Return a valid Arrow `SchemaRef` representing this `Schema`
    pub fn as_arrow(&self) -> ArrowSchemaRef {
        Arc::clone(&self.inner)
    }

    /// Create and validate a new Schema, creating metadata to
    /// represent the the various parts. This method is intended to be
    /// used only by the SchemaBuilder.
    pub(crate) fn new_from_parts(
        measurement: Option<String>,
        fields: impl Iterator<Item = (ArrowField, Option<InfluxColumnType>)>,
        sort_columns: bool,
    ) -> Result<Self> {
        let mut metadata = HashMap::new();

        if let Some(measurement) = measurement {
            metadata.insert(MEASUREMENT_METADATA_KEY.to_string(), measurement);
        }

        let mut fields: Vec<ArrowField> = fields
            .map(|(mut field, column_type)| {
                set_field_metadata(&mut field, column_type);
                field
            })
            .collect();

        if sort_columns {
            fields.sort_unstable_by(|a, b| a.name().cmp(b.name()));
        }

        // Call new_from_arrow to do normal, additional validation
        // (like dupe column detection)
        let record =
            ArrowSchemaRef::new(ArrowSchema::new_with_metadata(fields, metadata)).try_into()?;

        Ok(record)
    }

    /// Returns true if the sort_key includes all primary key cols
    pub fn is_sorted_on_pk(&self, sort_key: &SortKey) -> bool {
        self.primary_key().iter().all(|col| sort_key.contains(col))
    }

    /// Provide a reference to the underlying Arrow Schema object
    pub fn inner(&self) -> &ArrowSchemaRef {
        &self.inner
    }

    /// Return the InfluxDB data model type, if any, and underlying arrow
    /// schema field for the column at index `idx`. Panics if `idx` is
    /// greater than or equal to self.len()
    ///
    /// if there is no corresponding influx metadata,
    /// returns None for the influxdb_column_type
    pub fn field(&self, idx: usize) -> (Option<InfluxColumnType>, &ArrowField) {
        let field = self.inner.field(idx);
        (get_influx_type(field), field)
    }

    /// Find the index of the column with the given name, if any.
    pub fn find_index_of(&self, name: &str) -> Option<usize> {
        self.inner.index_of(name).ok()
    }

    /// Provides the InfluxDB data model measurement name for this schema, if
    /// any
    pub fn measurement(&self) -> Option<&String> {
        self.inner.metadata().get(MEASUREMENT_METADATA_KEY)
    }

    /// Returns the number of columns defined in this schema
    pub fn len(&self) -> usize {
        self.inner.fields().len()
    }

    /// Returns `true` if the schema contains no fields.
    pub fn is_empty(&self) -> bool {
        self.inner.fields().is_empty()
    }

    /// Returns an iterator of (Option<InfluxColumnType>, &Field) for
    /// all the columns of this schema, in order
    pub fn iter(&self) -> SchemaIter<'_> {
        SchemaIter::new(self)
    }

    /// Returns an iterator of `&Field` for all the tag columns of
    /// this schema, in order
    pub fn tags_iter(&self) -> impl Iterator<Item = &ArrowField> {
        self.iter().filter_map(|(influx_column_type, field)| {
            if matches!(influx_column_type, Some(InfluxColumnType::Tag)) {
                Some(field)
            } else {
                None
            }
        })
    }

    /// Returns an iterator of `&Field` for all the field columns of
    /// this schema, in order
    pub fn fields_iter(&self) -> impl Iterator<Item = &ArrowField> {
        self.iter().filter_map(|(influx_column_type, field)| {
            if matches!(influx_column_type, Some(InfluxColumnType::Field(_))) {
                Some(field)
            } else {
                None
            }
        })
    }

    /// Returns an iterator of `&Field` for all the timestamp columns
    /// of this schema, in order. At the time of writing there should
    /// be only one or 0 such columns
    pub fn time_iter(&self) -> impl Iterator<Item = &ArrowField> {
        self.iter().filter_map(|(influx_column_type, field)| {
            if matches!(influx_column_type, Some(InfluxColumnType::Timestamp)) {
                Some(field)
            } else {
                None
            }
        })
    }

    /// Resort order of our columns lexicographically by name
    pub fn sort_fields_by_name(self) -> Self {
        // pairs of (orig_index, field_ref)
        let mut sorted_fields: Vec<(usize, &ArrowField)> =
            self.inner.fields().iter().enumerate().collect();
        sorted_fields.sort_by(|a, b| a.1.name().cmp(b.1.name()));

        let is_sorted = sorted_fields
            .iter()
            .enumerate()
            .all(|(index, pair)| index == pair.0);

        if is_sorted {
            self
        } else {
            // No way at present to destructure an existing Schema so
            // we have to copy :(
            let new_fields: Vec<ArrowField> =
                sorted_fields.iter().map(|pair| pair.1).cloned().collect();

            let new_meta = self.inner.metadata().clone();
            let new_schema = ArrowSchema::new_with_metadata(new_fields, new_meta);

            Self {
                inner: Arc::new(new_schema),
            }
        }
    }

    /// Returns a Schema that represents selecting some of the columns
    /// in this schema. An error is returned if the selection refers to
    /// columns that do not exist.
    pub fn select(&self, selection: Selection<'_>) -> Result<Self> {
        Ok(match self.df_projection(selection)? {
            None => self.clone(),
            Some(indicies) => self.select_by_indices(&indicies),
        })
    }

    /// Returns a DataFusion style "projection" when the selection is
    /// applied to this schema.
    ///
    /// * `None` means "all columns"
    /// * `Some(indicies)` means the subset
    pub fn df_projection(&self, selection: Selection<'_>) -> Result<Option<Vec<usize>>> {
        Ok(match selection {
            Selection::All => None,
            Selection::Some(columns) => {
                let projection = columns
                    .iter()
                    .map(|&column_name| {
                        self.find_index_of(column_name)
                            .context(ColumnNotFoundSnafu { column_name })
                    })
                    .collect::<Result<Vec<_>>>()?;
                Some(projection)
            }
        })
    }

    /// Returns a Schema for the given (sub)set of column projects
    pub fn select_by_indices(&self, selection: &[usize]) -> Self {
        let mut fields = Vec::with_capacity(selection.len());
        for idx in selection {
            let field = self.inner.field(*idx);
            fields.push(field.clone());
        }

        let mut metadata = HashMap::with_capacity(1);
        if let Some(measurement) = self.inner.metadata().get(MEASUREMENT_METADATA_KEY).cloned() {
            metadata.insert(MEASUREMENT_METADATA_KEY.to_string(), measurement);
        }

        Self {
            inner: Arc::new(ArrowSchema::new_with_metadata(fields, metadata)),
        }
    }

    /// Returns a Schema for a given (sub)set of named columns
    pub fn select_by_names(&self, selection: &[&str]) -> Result<Self> {
        self.select(Selection::Some(selection))
    }

    /// Return columns used for the "primary key" in this table.
    ///
    /// Currently this relies on the InfluxDB data model annotations
    /// for what columns to include in the key columns
    pub fn primary_key(&self) -> Vec<&str> {
        use InfluxColumnType::*;
        let mut primary_keys: Vec<_> = self
            .iter()
            .filter_map(|(column_type, field)| match column_type {
                Some(Tag) => Some((Tag, field)),
                Some(Field(_)) => None,
                Some(Timestamp) => Some((Timestamp, field)),
                None => None,
            })
            .collect();

        // Now, sort lexographically (but put timestamp last)
        primary_keys.sort_by(|(a_column_type, a), (b_column_type, b)| {
            match (a_column_type, b_column_type) {
                (Tag, Tag) => a.name().cmp(b.name()),
                (Timestamp, Tag) => Ordering::Greater,
                (Tag, Timestamp) => Ordering::Less,
                (Timestamp, Timestamp) => panic!("multiple timestamps in summary"),
                _ => panic!("Unexpected types in key summary"),
            }
        });

        // Take just the names
        primary_keys
            .into_iter()
            .map(|(_column_type, field)| field.name().as_str())
            .collect()
    }

    /// Estimate memory consumption in bytes of the schema.
    ///
    /// This includes the size of `Self` as well as the inner [`Arc`]ed arrow schema.
    pub fn estimate_size(&self) -> usize {
        let size_self = size_of_val(self);

        let size_inner = size_of_val(self.inner.as_ref());

        let fields = self.inner.fields();
        let size_fields = fields.capacity() * size_of::<arrow::datatypes::Field>()
            + fields
                .iter()
                .map(|field| {
                    field.name().capacity()
                        + field
                            .metadata()
                            .map(|md| {
                                md.iter()
                                    .map(|(k, v)| k.capacity() + v.capacity())
                                    .sum::<usize>()
                            })
                            .unwrap_or_default()
                })
                .sum::<usize>();

        let metadata = self.inner.metadata();
        let size_metadata = metadata.capacity() * size_of::<(String, String)>()
            + metadata
                .iter()
                .map(|(k, v)| k.capacity() + v.capacity())
                .sum::<usize>();

        size_self + size_inner + size_fields + size_metadata
    }
}

/// Gets the influx type for a field
pub(crate) fn get_influx_type(field: &ArrowField) -> Option<InfluxColumnType> {
    field
        .metadata()
        .as_ref()?
        .get(COLUMN_METADATA_KEY)?
        .as_str()
        .try_into()
        .ok()
}

/// Sets the metadata for a field - replacing any existing metadata
pub(crate) fn set_field_metadata(field: &mut ArrowField, column_type: Option<InfluxColumnType>) {
    let mut metadata = std::collections::BTreeMap::new();

    if let Some(column_type) = column_type {
        metadata.insert(COLUMN_METADATA_KEY.to_string(), column_type.to_string());
    }

    field.set_metadata(Some(metadata))
}

/// Field value types for InfluxDB 2.0 data model, as defined in
/// [the documentation]: <https://docs.influxdata.com/influxdb/v2.0/reference/syntax/line-protocol/>
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum InfluxFieldType {
    /// 64-bit floating point number (TDB if NULLs / Nans are allowed)
    Float,
    /// 64-bit signed integer
    Integer,
    /// Unsigned 64-bit integers
    UInteger,
    /// UTF-8 encoded string
    String,
    /// true or false
    Boolean,
}

impl From<InfluxFieldType> for ArrowDataType {
    fn from(t: InfluxFieldType) -> Self {
        match t {
            InfluxFieldType::Float => Self::Float64,
            InfluxFieldType::Integer => Self::Int64,
            InfluxFieldType::UInteger => Self::UInt64,
            InfluxFieldType::String => Self::Utf8,
            InfluxFieldType::Boolean => Self::Boolean,
        }
    }
}

impl TryFrom<ArrowDataType> for InfluxFieldType {
    type Error = &'static str;

    fn try_from(value: ArrowDataType) -> Result<Self, Self::Error> {
        match value {
            ArrowDataType::Float64 => Ok(Self::Float),
            ArrowDataType::Int64 => Ok(Self::Integer),
            ArrowDataType::UInt64 => Ok(Self::UInteger),
            ArrowDataType::Utf8 => Ok(Self::String),
            ArrowDataType::Boolean => Ok(Self::Boolean),
            _ => Err("No corresponding type in the InfluxDB data model"),
        }
    }
}

/// Column types.
///
/// Includes types for tags and fields in the InfluxDB data model, as described in the
/// [documentation](https://docs.influxdata.com/influxdb/v2.0/reference/syntax/line-protocol/).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum InfluxColumnType {
    /// Tag
    ///
    /// Note: tags are always stored as a Utf8, but eventually this
    /// should allow for both Utf8 and Dictionary
    Tag,

    /// Field: Data of type in InfluxDB Data model
    Field(InfluxFieldType),

    /// Timestamp
    ///
    /// 64 bit timestamp "UNIX timestamps" representing nanosecods
    /// since the UNIX epoch (00:00:00 UTC on 1 January 1970).
    Timestamp,
}

impl InfluxColumnType {
    /// returns true if `arrow_type` can validly store this column type
    pub fn valid_arrow_type(&self, data_type: &ArrowDataType) -> bool {
        match self {
            Self::Tag => match data_type {
                ArrowDataType::Utf8 => true,
                ArrowDataType::Dictionary(key, value) => {
                    key.as_ref() == &ArrowDataType::Int32 && value.as_ref() == &ArrowDataType::Utf8
                }
                _ => false,
            },
            Self::Field(_) | Self::Timestamp => {
                let default_type: ArrowDataType = self.into();
                data_type == &default_type
            }
        }
    }
}

/// "serialization" to strings that are stored in arrow metadata
impl From<&InfluxColumnType> for &'static str {
    fn from(t: &InfluxColumnType) -> Self {
        match t {
            InfluxColumnType::Tag => "iox::column_type::tag",
            InfluxColumnType::Field(InfluxFieldType::Float) => "iox::column_type::field::float",
            InfluxColumnType::Field(InfluxFieldType::Integer) => "iox::column_type::field::integer",
            InfluxColumnType::Field(InfluxFieldType::UInteger) => {
                "iox::column_type::field::uinteger"
            }
            InfluxColumnType::Field(InfluxFieldType::String) => "iox::column_type::field::string",
            InfluxColumnType::Field(InfluxFieldType::Boolean) => "iox::column_type::field::boolean",
            InfluxColumnType::Timestamp => "iox::column_type::timestamp",
        }
    }
}

impl std::fmt::Display for InfluxColumnType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s: &str = self.into();
        write!(f, "{}", s)
    }
}

/// "deserialization" from strings that are stored in arrow metadata
impl TryFrom<&str> for InfluxColumnType {
    type Error = String;
    /// this is the inverse of converting to &str
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        match s {
            "iox::column_type::tag" => Ok(Self::Tag),
            "iox::column_type::field::float" => Ok(Self::Field(InfluxFieldType::Float)),
            "iox::column_type::field::integer" => Ok(Self::Field(InfluxFieldType::Integer)),
            "iox::column_type::field::uinteger" => Ok(Self::Field(InfluxFieldType::UInteger)),
            "iox::column_type::field::string" => Ok(Self::Field(InfluxFieldType::String)),
            "iox::column_type::field::boolean" => Ok(Self::Field(InfluxFieldType::Boolean)),
            "iox::column_type::timestamp" => Ok(Self::Timestamp),
            _ => Err(format!("Unknown column type in metadata: {:?}", s)),
        }
    }
}

impl From<&InfluxColumnType> for ArrowDataType {
    /// What arrow type is used for this column type?
    fn from(t: &InfluxColumnType) -> Self {
        match t {
            InfluxColumnType::Tag => Self::Dictionary(Box::new(Self::Int32), Box::new(Self::Utf8)),
            InfluxColumnType::Field(influxdb_field_type) => (*influxdb_field_type).into(),
            InfluxColumnType::Timestamp => TIME_DATA_TYPE(),
        }
    }
}

/// Thing that implements iterator over a Schema's columns.
pub struct SchemaIter<'a> {
    schema: &'a Schema,
    idx: usize,
}

impl<'a> SchemaIter<'a> {
    fn new(schema: &'a Schema) -> Self {
        Self { schema, idx: 0 }
    }
}

impl<'a> fmt::Debug for SchemaIter<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SchemaIter<{}>", self.idx)
    }
}

impl<'a> Iterator for SchemaIter<'a> {
    type Item = (Option<InfluxColumnType>, &'a ArrowField);

    fn next(&mut self) -> Option<Self::Item> {
        if self.idx < self.schema.len() {
            let ret = self.schema.field(self.idx);
            self.idx += 1;
            Some(ret)
        } else {
            None
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.schema.len()))
    }
}

/// Asserts that the result of calling Schema:field(i) is as expected:
///
/// example
///   assert_column_eq!(schema, 0, InfluxColumnType::Tag, "host");
#[macro_export]
macro_rules! assert_column_eq {
    ($schema:expr, $i:expr, $expected_influxdb_column_type:expr, $expected_field_name:expr) => {
        let (influxdb_column_type, arrow_field) = $schema.field($i);
        assert_eq!(
            influxdb_column_type,
            Some($expected_influxdb_column_type),
            "Line protocol column mismatch for column {}, field {:?}, in schema {:#?}",
            $i,
            arrow_field,
            $schema
        );
        assert_eq!(
            arrow_field.name(),
            $expected_field_name,
            "expected field name mismatch for column {}, field {:?}, in schema {:#?}",
            $i,
            arrow_field,
            $schema
        )
    };
}

#[cfg(test)]
mod test {
    use InfluxColumnType::*;
    use InfluxFieldType::*;

    use super::{builder::SchemaBuilder, *};

    fn make_field(
        name: &str,
        data_type: arrow::datatypes::DataType,
        nullable: bool,
        column_type: &str,
    ) -> ArrowField {
        let mut field = ArrowField::new(name, data_type, nullable);
        field.set_metadata(Some(
            vec![(COLUMN_METADATA_KEY.to_string(), column_type.to_string())]
                .into_iter()
                .collect(),
        ));
        field
    }

    #[test]
    fn new_from_arrow_no_metadata() {
        let arrow_schema = ArrowSchemaRef::new(ArrowSchema::new(vec![
            ArrowField::new("col1", ArrowDataType::Int64, false),
            ArrowField::new("col2", ArrowDataType::Utf8, false),
        ]));

        // Given a schema created from arrow record batch with no metadata
        let schema: Schema = Arc::clone(&arrow_schema).try_into().unwrap();
        assert_eq!(schema.len(), 2);

        // It still works, but has no lp column types
        let (influxdb_column_type, field) = schema.field(0);
        assert_eq!(field.name(), "col1");
        assert_eq!(field, arrow_schema.field(0));
        assert_eq!(influxdb_column_type, None);

        let (influxdb_column_type, field) = schema.field(1);
        assert_eq!(field.name(), "col2");
        assert_eq!(field, arrow_schema.field(1));
        assert_eq!(influxdb_column_type, None);
    }

    #[test]
    fn new_from_arrow_metadata_good() {
        let fields = vec![
            make_field(
                "tag_col",
                ArrowDataType::Utf8,
                false,
                "iox::column_type::tag",
            ),
            make_field(
                "int_col",
                ArrowDataType::Int64,
                false,
                "iox::column_type::field::integer",
            ),
            make_field(
                "uint_col",
                ArrowDataType::UInt64,
                false,
                "iox::column_type::field::uinteger",
            ),
            make_field(
                "float_col",
                ArrowDataType::Float64,
                false,
                "iox::column_type::field::float",
            ),
            make_field(
                "str_col",
                ArrowDataType::Utf8,
                false,
                "iox::column_type::field::string",
            ),
            make_field(
                "bool_col",
                ArrowDataType::Boolean,
                false,
                "iox::column_type::field::boolean",
            ),
            make_field(
                "time_col",
                TIME_DATA_TYPE(),
                false,
                "iox::column_type::timestamp",
            ),
        ];

        let metadata: HashMap<_, _> = vec![(
            "iox::measurement::name".to_string(),
            "the_measurement".to_string(),
        )]
        .into_iter()
        .collect();

        let arrow_schema = ArrowSchemaRef::new(ArrowSchema::new_with_metadata(fields, metadata));

        let schema: Schema = arrow_schema.try_into().unwrap();
        assert_column_eq!(schema, 0, Tag, "tag_col");
        assert_column_eq!(schema, 1, Field(Integer), "int_col");
        assert_column_eq!(schema, 2, Field(UInteger), "uint_col");
        assert_column_eq!(schema, 3, Field(Float), "float_col");
        assert_column_eq!(schema, 4, Field(String), "str_col");
        assert_column_eq!(schema, 5, Field(Boolean), "bool_col");
        assert_column_eq!(schema, 6, Timestamp, "time_col");
        assert_eq!(schema.len(), 7);

        assert_eq!(schema.measurement().unwrap(), "the_measurement");
    }

    #[test]
    fn new_from_arrow_metadata_extra() {
        let fields = vec![
            make_field(
                "tag_col",
                ArrowDataType::Utf8,
                false,
                "something_other_than_iox",
            ),
            make_field(
                "int_col",
                ArrowDataType::Int64,
                false,
                "iox::column_type::field::some_new_exotic_type",
            ),
        ];

        // This metadata models metadata that was not created by this
        // rust module itself
        let metadata: HashMap<_, _> = vec![("iox::some::new::key".to_string(), "foo".to_string())]
            .into_iter()
            .collect();

        let arrow_schema = ArrowSchemaRef::new(ArrowSchema::new_with_metadata(fields, metadata));

        // Having this succeed is the primary test
        let schema: Schema = arrow_schema.try_into().unwrap();

        let (influxdb_column_type, field) = schema.field(0);
        assert_eq!(field.name(), "tag_col");
        assert_eq!(influxdb_column_type, None);

        let (influxdb_column_type, field) = schema.field(1);
        assert_eq!(field.name(), "int_col");
        assert_eq!(influxdb_column_type, None);
    }

    // mismatched metadata / arrow types
    #[test]
    fn new_from_arrow_metadata_mismatched_tag() {
        let fields = vec![
            make_field(
                "tag_col",
                ArrowDataType::Int64,
                false,
                "iox::column_type::tag",
            ), // not a valid tag type
        ];

        let arrow_schema = ArrowSchemaRef::new(ArrowSchema::new(fields));

        let res = Schema::try_from_arrow(arrow_schema);
        assert_eq!(res.unwrap_err().to_string(), "Error: Incompatible metadata type found in schema for column 'tag_col'. Metadata specified Tag which is incompatible with actual type Int64");
    }

    // mismatched metadata / arrow types
    #[test]
    fn new_from_arrow_metadata_mismatched_field() {
        let fields = vec![make_field(
            "int_col",
            ArrowDataType::Int64,
            false,
            "iox::column_type::field::float",
        )];
        let arrow_schema = ArrowSchemaRef::new(ArrowSchema::new(fields));

        let res = Schema::try_from_arrow(arrow_schema);
        assert_eq!(res.unwrap_err().to_string(), "Error: Incompatible metadata type found in schema for column 'int_col'. Metadata specified Field(Float) which is incompatible with actual type Int64");
    }

    // mismatched metadata / arrow types
    #[test]
    fn new_from_arrow_metadata_mismatched_timestamp() {
        let fields = vec![
            make_field(
                "time",
                ArrowDataType::Utf8,
                false,
                "iox::column_type::timestamp",
            ), // timestamp can't be strings
        ];

        let arrow_schema = ArrowSchemaRef::new(ArrowSchema::new(fields));

        let res = Schema::try_from_arrow(arrow_schema);
        assert_eq!(res.unwrap_err().to_string(), "Error: Incompatible metadata type found in schema for column 'time'. Metadata specified Timestamp which is incompatible with actual type Utf8");
    }

    #[test]
    fn new_from_arrow_replicated_columns() {
        // arrow allows duplicated colum names
        let fields = vec![
            ArrowField::new("the_column", ArrowDataType::Utf8, false),
            ArrowField::new("another_column", ArrowDataType::Utf8, false),
            ArrowField::new("the_column", ArrowDataType::Utf8, false),
        ];

        let arrow_schema = ArrowSchemaRef::new(ArrowSchema::new(fields));

        let res = Schema::try_from_arrow(arrow_schema);
        assert_eq!(
            res.unwrap_err().to_string(),
            "Error: Duplicate column name found in schema: 'the_column'"
        );
    }

    #[test]
    fn test_round_trip() {
        let schema1 = SchemaBuilder::new()
            .influx_field("the_field", String)
            .tag("the_tag")
            .timestamp()
            .measurement("the_measurement")
            .build()
            .unwrap();

        // Make a new schema via ArrowSchema (serialized metadata) to ensure that
        // the metadata makes it through a round trip

        let arrow_schema_1: ArrowSchemaRef = schema1.clone().into();
        let schema2 = Schema::try_from_arrow(arrow_schema_1).unwrap();

        for s in &[schema1, schema2] {
            assert_eq!(s.measurement().unwrap(), "the_measurement");
            assert_column_eq!(s, 0, Field(String), "the_field");
            assert_column_eq!(s, 1, Tag, "the_tag");
            assert_column_eq!(s, 2, Timestamp, "time");
            assert_eq!(3, s.len());
        }
    }

    /// Build an empty iterator
    fn empty_schema() -> Schema {
        SchemaBuilder::new().build().unwrap()
    }

    #[test]
    fn test_iter_empty() {
        assert_eq!(empty_schema().iter().count(), 0);
    }

    #[test]
    fn test_tags_iter_empty() {
        assert_eq!(empty_schema().tags_iter().count(), 0);
    }

    #[test]
    fn test_fields_iter_empty() {
        assert_eq!(empty_schema().fields_iter().count(), 0);
    }

    #[test]
    fn test_time_iter_empty() {
        assert_eq!(empty_schema().time_iter().count(), 0);
    }

    /// Build a schema for testing iterators
    fn iter_schema() -> Schema {
        SchemaBuilder::new()
            .influx_field("field1", Float)
            .tag("tag1")
            .timestamp()
            .influx_field("field2", String)
            .influx_field("field3", String)
            .tag("tag2")
            .build()
            .unwrap()
    }

    #[test]
    fn test_iter() {
        let schema = iter_schema();

        // test schema iterator and field accessor match up
        for (i, (iter_col_type, iter_field)) in schema.iter().enumerate() {
            let (col_type, field) = schema.field(i);
            assert_eq!(iter_col_type, col_type);
            assert_eq!(iter_field, field);
        }
        assert_eq!(schema.iter().count(), 6);
    }

    #[test]
    fn test_tags_iter() {
        let schema = iter_schema();

        let mut iter = schema.tags_iter();
        assert_eq!(iter.next().unwrap().name(), "tag1");
        assert_eq!(iter.next().unwrap().name(), "tag2");
        assert_eq!(iter.next(), None);
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn test_fields_iter() {
        let schema = iter_schema();

        let mut iter = schema.fields_iter();
        assert_eq!(iter.next().unwrap().name(), "field1");
        assert_eq!(iter.next().unwrap().name(), "field2");
        assert_eq!(iter.next().unwrap().name(), "field3");
        assert_eq!(iter.next(), None);
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn test_time_iter() {
        let schema = iter_schema();

        let mut iter = schema.time_iter();
        assert_eq!(iter.next().unwrap().name(), "time");
        assert_eq!(iter.next(), None);
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn test_sort_fields_by_name_already_sorted() {
        let schema = SchemaBuilder::new()
            .field("field_a", ArrowDataType::Int64)
            .field("field_b", ArrowDataType::Int64)
            .field("field_c", ArrowDataType::Int64)
            .build()
            .unwrap();

        let sorted_schema = schema.clone().sort_fields_by_name();

        assert_eq!(
            schema, sorted_schema,
            "\nExpected:\n{:#?}\nActual:\n{:#?}",
            schema, sorted_schema
        );
    }

    #[test]
    fn test_sort_fields_by_name() {
        let schema = SchemaBuilder::new()
            .field("field_b", ArrowDataType::Int64)
            .field("field_a", ArrowDataType::Int64)
            .field("field_c", ArrowDataType::Int64)
            .build()
            .unwrap();

        let sorted_schema = schema.sort_fields_by_name();

        let expected_schema = SchemaBuilder::new()
            .field("field_a", ArrowDataType::Int64)
            .field("field_b", ArrowDataType::Int64)
            .field("field_c", ArrowDataType::Int64)
            .build()
            .unwrap();

        assert_eq!(
            expected_schema, sorted_schema,
            "\nExpected:\n{:#?}\nActual:\n{:#?}",
            expected_schema, sorted_schema
        );
    }

    #[test]
    fn test_select() {
        let schema1 = SchemaBuilder::new()
            .influx_field("the_field", String)
            .tag("the_tag")
            .timestamp()
            .measurement("the_measurement")
            .build()
            .unwrap();

        let schema2 = schema1.select_by_names(&[TIME_COLUMN_NAME]).unwrap();
        let schema3 = Schema::try_from_arrow(Arc::clone(&schema2.inner)).unwrap();

        assert_eq!(schema1.measurement(), schema2.measurement());
        assert_eq!(schema1.measurement(), schema3.measurement());

        assert_eq!(schema1.len(), 3);
        assert_eq!(schema2.len(), 1);
        assert_eq!(schema3.len(), 1);

        assert_eq!(schema1.inner.fields().len(), 3);
        assert_eq!(schema2.inner.fields().len(), 1);
        assert_eq!(schema3.inner.fields().len(), 1);

        let get_type = |x: &Schema, field: &str| -> InfluxColumnType {
            let idx = x.find_index_of(field).unwrap();
            x.field(idx).0.unwrap()
        };

        assert_eq!(
            get_type(&schema1, TIME_COLUMN_NAME),
            InfluxColumnType::Timestamp
        );
        assert_eq!(
            get_type(&schema2, TIME_COLUMN_NAME),
            InfluxColumnType::Timestamp
        );
        assert_eq!(get_type(&schema1, "the_tag"), InfluxColumnType::Tag);
        assert_eq!(
            get_type(&schema1, "the_field"),
            InfluxColumnType::Field(InfluxFieldType::String)
        );
        assert_eq!(
            get_type(&schema2, TIME_COLUMN_NAME),
            InfluxColumnType::Timestamp
        );
        assert_eq!(
            get_type(&schema3, TIME_COLUMN_NAME),
            InfluxColumnType::Timestamp
        );
    }

    #[test]
    fn test_df_projection() {
        let schema = SchemaBuilder::new()
            .influx_field("the_field", String)
            .tag("the_tag")
            .timestamp()
            .measurement("the_measurement")
            .build()
            .unwrap();

        assert_eq!(schema.df_projection(Selection::All).unwrap(), None);
        assert_eq!(
            schema.df_projection(Selection::Some(&["the_tag"])).unwrap(),
            Some(vec![1])
        );
        assert_eq!(
            schema
                .df_projection(Selection::Some(&["the_tag", "the_field"]))
                .unwrap(),
            Some(vec![1, 0])
        );

        let res = schema.df_projection(Selection::Some(&["the_tag", "unknown_field"]));
        assert_eq!(
            res.unwrap_err().to_string(),
            "Column not found 'unknown_field'"
        );
    }

    #[test]
    fn test_is_sort_on_pk() {
        // Sort key the same as pk
        let sort_key =
            SortKey::from_columns(vec!["tag4", "tag3", "tag2", "tag1", TIME_COLUMN_NAME]);

        let schema = SchemaBuilder::new()
            .influx_field("the_field", String)
            .tag("tag1")
            .tag("tag2")
            .tag("tag3")
            .tag("tag4")
            .timestamp()
            .measurement("the_measurement")
            .build()
            .unwrap();
        assert!(schema.is_sorted_on_pk(&sort_key));

        // Sort key does not include all pk cols
        let sort_key = SortKey::from_columns(vec!["tag3", "tag1", TIME_COLUMN_NAME]);

        let schema = SchemaBuilder::new()
            .influx_field("the_field", String)
            .tag("tag1")
            .tag("tag2")
            .tag("tag3")
            .tag("tag4")
            .timestamp()
            .measurement("the_measurement")
            .build()
            .unwrap();
        assert!(!schema.is_sorted_on_pk(&sort_key));

        // No PK, sort key on non pk
        let sort_key = SortKey::from_columns(vec!["the_field"]);

        let schema = SchemaBuilder::new()
            .influx_field("the_field", String)
            .tag("tag1")
            .tag("tag2")
            .tag("tag3")
            .tag("tag4")
            .timestamp()
            .measurement("the_measurement")
            .build()
            .unwrap();
        assert!(!schema.is_sorted_on_pk(&sort_key));
    }

    #[test]
    fn test_estimate_size() {
        let schema = SchemaBuilder::new()
            .influx_field("the_field", String)
            .tag("the_tag")
            .timestamp()
            .measurement("the_measurement")
            .build()
            .unwrap();

        // this is mostly a smoke test
        assert_eq!(schema.estimate_size(), 795);
    }
}
