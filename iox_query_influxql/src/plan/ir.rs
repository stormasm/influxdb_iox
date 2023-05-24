//! Defines data structures which represent an InfluxQL
//! statement after it has been processed

use crate::plan::rewriter::ProjectionType;
use influxdb_influxql_parser::common::{
    LimitClause, MeasurementName, OffsetClause, OrderByClause, QualifiedMeasurementName,
    WhereClause,
};
use influxdb_influxql_parser::expression::Expr;
use influxdb_influxql_parser::select::{
    FieldList, FillClause, FromMeasurementClause, GroupByClause, MeasurementSelection,
    SelectStatement, TimeZoneClause,
};
use schema::InfluxColumnType;
use std::collections::HashSet;
use std::fmt::{Display, Formatter};

/// A set of tag keys.
pub(super) type TagSet = HashSet<String>;

/// Represents a validated and normalized top-level [`SelectStatement`].
#[derive(Debug, Default, Clone)]
pub(super) struct SelectQuery {
    pub(super) select: Select,
}

#[derive(Debug, Default, Clone)]
pub(super) struct Select {
    /// The projection type of the selection.
    pub(super) projection_type: ProjectionType,

    /// Projection clause of the selection.
    pub(super) fields: Vec<Field>,

    /// A list of data sources for the selection.
    pub(super) from: Vec<DataSource>,

    /// A conditional expression to filter the selection.
    pub(super) condition: Option<WhereClause>,

    /// The GROUP BY clause of the selection.
    pub(super) group_by: Option<GroupByClause>,

    /// The set of possible tags for the selection, by combining
    /// the tag sets of all inputs via the `FROM` clause.
    pub(super) tag_set: TagSet,

    /// The [fill] clause specifies the fill behaviour for the selection. If the value is [`None`],
    /// it is the same behavior as `fill(null)`.
    ///
    /// [fill]: https://docs.influxdata.com/influxdb/v1.8/query_language/explore-data/#group-by-time-intervals-and-fill
    pub(super) fill: Option<FillClause>,

    /// Configures the ordering of the selection by time.
    pub(super) order_by: Option<OrderByClause>,

    /// A value to restrict the number of rows returned.
    pub(super) limit: Option<LimitClause>,

    /// A value to specify an offset to start retrieving rows.
    pub(super) offset: Option<OffsetClause>,

    /// The timezone for the query, specified as [`tz('<time zone>')`][time_zone_clause].
    ///
    /// [time_zone_clause]: https://docs.influxdata.com/influxdb/v1.8/query_language/explore-data/#the-time-zone-clause
    pub(super) timezone: Option<chrono_tz::Tz>,
}

impl From<Select> for SelectStatement {
    fn from(value: Select) -> Self {
        Self {
            fields: FieldList::new(
                value
                    .fields
                    .into_iter()
                    .map(|c| influxdb_influxql_parser::select::Field {
                        expr: c.expr,
                        alias: Some(c.name.into()),
                    })
                    .collect(),
            ),
            from: FromMeasurementClause::new(
                value
                    .from
                    .into_iter()
                    .map(|tr| match tr {
                        DataSource::Table(name) => {
                            MeasurementSelection::Name(QualifiedMeasurementName {
                                database: None,
                                retention_policy: None,
                                name: MeasurementName::Name(name.as_str().into()),
                            })
                        }
                        DataSource::Subquery(q) => {
                            MeasurementSelection::Subquery(Box::new((*q).into()))
                        }
                    })
                    .collect(),
            ),
            condition: value.condition,
            group_by: value.group_by,
            fill: value.fill,
            order_by: value.order_by,
            limit: value.limit,
            offset: value.offset,
            series_limit: None,
            series_offset: None,
            timezone: value.timezone.map(TimeZoneClause::new),
        }
    }
}

/// Represents a data source that is either a table or a subquery in a [`Select`] from clause.
#[derive(Debug, Clone)]
pub(super) enum DataSource {
    Table(String),
    Subquery(Box<Select>),
}

#[derive(Debug, Clone)]
pub(super) struct Field {
    pub(super) expr: Expr,
    pub(super) name: String,
    pub(super) data_type: Option<InfluxColumnType>,
}

impl Display for Field {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.expr, f)?;
        write!(f, " AS {}", self.name)
    }
}