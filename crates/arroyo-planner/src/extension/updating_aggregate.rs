use arrow_schema::{DataType, Field, Schema, TimeUnit};
use arroyo_datastream::logical::{LogicalEdge, LogicalEdgeType, LogicalNode, OperatorName};
use arroyo_rpc::{df::ArroyoSchema, grpc::api::UpdatingAggregateOperator, TIMESTAMP_FIELD};
use datafusion::common::{plan_err, DFSchemaRef, Result, TableReference};
use datafusion::logical_expr::{Extension, LogicalPlan, UserDefinedLogicalNodeCore};
use datafusion_proto::protobuf::{physical_plan_node::PhysicalPlanType, PhysicalPlanNode};
use std::sync::Arc;
use std::time::Duration;

use crate::builder::{NamedNode, SplitPlanOutput};

use super::{ArroyoExtension, IsRetractExtension, NodeWithIncomingEdges};
use arroyo_rpc::config::config;
use prost::Message;

pub(crate) const UPDATING_AGGREGATE_EXTENSION_NAME: &str = "UpdatingAggregateExtension";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct UpdatingAggregateExtension {
    pub(crate) aggregate: LogicalPlan,
    pub(crate) key_fields: Vec<usize>,
    pub(crate) final_calculation: LogicalPlan,
    pub(crate) timestamp_qualifier: Option<TableReference>,
    pub(crate) ttl: Duration,
}

impl UpdatingAggregateExtension {
    pub fn new(
        aggregate: LogicalPlan,
        key_fields: Vec<usize>,
        timestamp_qualifier: Option<TableReference>,
        ttl: Duration,
    ) -> Self {
        let final_calculation = LogicalPlan::Extension(Extension {
            node: Arc::new(IsRetractExtension::new(
                aggregate.clone(),
                timestamp_qualifier.clone(),
            )),
        });
        Self {
            aggregate,
            key_fields,
            final_calculation,
            timestamp_qualifier,
            ttl,
        }
    }
}

impl UserDefinedLogicalNodeCore for UpdatingAggregateExtension {
    fn name(&self) -> &str {
        UPDATING_AGGREGATE_EXTENSION_NAME
    }

    fn inputs(&self) -> Vec<&LogicalPlan> {
        vec![&self.aggregate]
    }

    fn schema(&self) -> &DFSchemaRef {
        self.final_calculation.schema()
    }

    fn expressions(&self) -> Vec<datafusion::prelude::Expr> {
        vec![]
    }

    fn fmt_for_explain(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "UpdatingAggregateExtension")
    }

    fn with_exprs_and_inputs(
        &self,
        _exprs: Vec<datafusion::prelude::Expr>,
        inputs: Vec<LogicalPlan>,
    ) -> Result<Self> {
        Ok(Self::new(
            inputs[0].clone(),
            self.key_fields.clone(),
            self.timestamp_qualifier.clone(),
            self.ttl,
        ))
    }
}

impl ArroyoExtension for UpdatingAggregateExtension {
    fn node_name(&self) -> Option<NamedNode> {
        None
    }

    fn plan_node(
        &self,
        planner: &crate::builder::Planner,
        index: usize,
        input_schemas: Vec<arroyo_rpc::df::ArroyoSchemaRef>,
    ) -> Result<super::NodeWithIncomingEdges> {
        if input_schemas.len() != 1 {
            return plan_err!(
                "UpdatingAggregateExtension requires exactly one input schema, found {}",
                input_schemas.len()
            );
        }

        let input_schema = input_schemas[0].clone();
        let SplitPlanOutput {
            partial_aggregation_plan,
            partial_schema,
            finish_plan,
        } = planner.split_physical_plan(self.key_fields.clone(), &self.aggregate, false)?;

        let mut state_fields = partial_schema.schema.fields().to_vec();
        state_fields[partial_schema.timestamp_index] = Arc::new(Field::new(
            TIMESTAMP_FIELD,
            DataType::Timestamp(TimeUnit::Nanosecond, None),
            false,
        ));

        let state_partial_schema = ArroyoSchema::new_keyed(
            Arc::new(Schema::new_with_metadata(
                state_fields,
                partial_schema.schema.metadata().clone(),
            )),
            partial_schema.timestamp_index,
            self.key_fields.clone(),
        );

        let mut state_final_fields = self
            .aggregate
            .schema()
            .fields()
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let timestamp_index = state_final_fields.len() - 1;
        state_final_fields[timestamp_index] = Arc::new(Field::new(
            TIMESTAMP_FIELD,
            DataType::Timestamp(TimeUnit::Nanosecond, None),
            false,
        ));
        let state_final_schema = ArroyoSchema::new_keyed(
            Arc::new(Schema::new_with_metadata(
                state_final_fields,
                self.aggregate.schema().metadata().clone(),
            )),
            timestamp_index,
            self.key_fields.clone(),
        );

        let Some(PhysicalPlanType::Aggregate(aggregate)) = finish_plan.physical_plan_type.as_ref()
        else {
            return plan_err!("expect finish plan to be an aggregate");
        };

        let mut combine_aggregate = aggregate.as_ref().clone();
        combine_aggregate.set_mode(datafusion_proto::protobuf::AggregateMode::CombinePartial);
        let combine_plan = PhysicalPlanNode {
            physical_plan_type: Some(PhysicalPlanType::Aggregate(Box::new(combine_aggregate))),
        };

        let config = UpdatingAggregateOperator {
            name: "UpdatingAggregate".to_string(),
            partial_schema: Some(partial_schema.into()),
            state_partial_schema: Some(state_partial_schema.into()),
            state_final_schema: Some(state_final_schema.into()),
            partial_aggregation_plan: partial_aggregation_plan.encode_to_vec(),
            combine_plan: combine_plan.encode_to_vec(),
            final_aggregation_plan: finish_plan.encode_to_vec(),
            flush_interval_micros: config()
                .pipeline
                .update_aggregate_flush_interval
                .as_micros() as u64,
            ttl_micros: self.ttl.as_micros() as u64,
        };

        let node = LogicalNode {
            operator_id: format!("updating_aggregate_{}", index),
            description: "UpdatingAggregate".to_string(),
            operator_name: OperatorName::UpdatingAggregate,
            operator_config: config.encode_to_vec(),
            parallelism: 1,
        };

        let edge = LogicalEdge::project_all(LogicalEdgeType::Shuffle, (*input_schema).clone());

        Ok(NodeWithIncomingEdges {
            node,
            edges: vec![edge],
        })
    }

    fn output_schema(&self) -> arroyo_rpc::df::ArroyoSchema {
        ArroyoSchema::from_schema_unkeyed(Arc::new(self.schema().as_ref().into())).unwrap()
    }
}
