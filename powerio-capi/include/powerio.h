/* PowerIO C ABI 7.
 *
 * Grid exchange data enters through PioSource and pio_parse. pio_emit writes
 * a selected grid exchange format. PowerIO IR uses pio_module_serialize and
 * pio_module_deserialize. Every input and returned span carries a byte or
 * element count.
 *
 * Values use canonical structural type names. pio_value_is_type performs an
 * exact comparison, and the typed accessors return owner-rooted views without
 * serializing or copying the module value.
 *
 * Each opaque handle has retain and release functions. A child value,
 * network, collection entry, or artifact keeps its owner alive. Concurrent
 * immutable access is allowed; releasing one raw handle concurrently with a
 * call that uses that same raw handle is caller error. pio_apply_updates and
 * pio_apply_bus_load_active_power need exclusive access to their PioModule
 * handle for the duration of the call, and they invalidate every plain
 * Pio*View struct previously read from that handle; owner rooted handles
 * stay valid. Every pointer a caller passes must be aligned for its type and
 * must address at most PTRDIFF_MAX bytes.
 *
 * Every fallible operation reports through PioError **. A NULL error
 * parameter discards the error. Branch on pio_error_code, not on message
 * text. No Rust panic crosses the ABI boundary.
 *
 * Generated from the Rust declarations with cbindgen using this file.
 */

#ifndef POWERIO_H
#define POWERIO_H

#include <stdarg.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
typedef struct PioArtifact PioArtifact;
typedef struct PioActivePower PioActivePower;
typedef struct PioAcOpfPreparation PioAcOpfPreparation;
typedef struct PioDcOpfPreparation PioDcOpfPreparation;
typedef struct PioApparentPower PioApparentPower;
typedef struct PioBalancedNetwork PioBalancedNetwork;
typedef struct PioCalculationUpdate PioCalculationUpdate;
typedef struct PioCalculationInstance PioCalculationInstance;
typedef struct PioCalculationSolution PioCalculationSolution;
typedef struct PioComponentId PioComponentId;
typedef struct PioDestination PioDestination;
typedef struct PioDetailedConnectivity PioDetailedConnectivity;
typedef struct PioDiagnostics PioDiagnostics;
typedef struct PioEmitResult PioEmitResult;
typedef struct PioError PioError;
typedef struct PioGeoApplyReport PioGeoApplyReport;
typedef struct PioGeoLayer PioGeoLayer;
typedef struct PioJsonValue PioJsonValue;
typedef struct PioModule PioModule;
typedef struct PioMulticonductorNetwork PioMulticonductorNetwork;
typedef struct PioNetworkUpdate PioNetworkUpdate;
typedef struct PioOperatingPoint PioOperatingPoint;
typedef struct PioOperatingPointUpdate PioOperatingPointUpdate;
typedef struct PioReactivePower PioReactivePower;
typedef struct PioScenarioSetHandle PioScenarioSetHandle;
typedef struct PioSource PioSource;
typedef struct PioSparseMatrix PioSparseMatrix;
typedef struct PioString PioString;
typedef struct PioTimeSeriesHandle PioTimeSeriesHandle;
typedef struct PioUpdateChange PioUpdateChange;
typedef struct PioUpdateReport PioUpdateReport;
typedef struct PioValueHandle PioValueHandle;
typedef struct PioVector PioVector;

/**
 * C ABI version.
 */
#define PIO_ABI_VERSION 7

/**
 * Borrowed UTF-8 bytes. The bytes need not end in NUL.
 */
typedef struct {
    const char *data;
    size_t len;
} PioStringView;

/**
 * One borrowed source byte range from a diagnostic.
 */
typedef struct {
    PioStringView source;
    uint64_t byte_start;
    uint64_t byte_end;
} PioDiagnosticSpanView;

/**
 * Program identity recorded with one module.
 */
typedef struct {
    PioStringView name;
    PioStringView version;
} PioModuleProducerView;

/**
 * One durable source descriptor recorded with a module.
 */
typedef struct {
    PioStringView id;
    PioStringView name;
    uint64_t byte_length;
    PioStringView format;
    bool has_format;
    PioStringView digest_algorithm;
    PioStringView digest;
    bool has_digest;
} PioModuleSourceView;

/**
 * One typed value target and its relation to source bytes.
 */
typedef struct {
    PioStringView target;
    PioStringView relation;
    size_t span_count;
} PioModuleSourceMapEntryView;

/**
 * One borrowed source byte range from a source map entry.
 */
typedef struct {
    PioStringView source;
    uint64_t byte_start;
    uint64_t byte_end;
} PioSourceSpanView;

/**
 * One operation recorded in module history.
 */
typedef struct {
    PioStringView id;
    PioStringView kind;
    PioStringView name;
    PioStringView input_type;
    bool has_input_type;
    PioStringView output_type;
    bool has_output_type;
    size_t parameter_count;
    size_t assumption_count;
    size_t loss_count;
} PioModuleHistoryEntryView;

/**
 * One named structured parameter in a module history entry.
 */
typedef struct {
    PioStringView name;
    PioStringView value_kind;
} PioModuleHistoryParameterView;

/**
 * One namespaced structured module extension.
 */
typedef struct {
    PioStringView namespace_;
    PioStringView value_kind;
} PioModuleExtensionView;

/**
 * One structured JSON value stored in module history or extensions.
 */
typedef struct {
    PioStringView kind;
    bool boolean_value;
    PioStringView number_kind;
    int64_t signed_integer_value;
    uint64_t unsigned_integer_value;
    double floating_point_value;
    PioStringView string_value;
    size_t element_count;
} PioJsonValueView;

/**
 * One key and value type in a structured JSON object.
 */
typedef struct {
    PioStringView key;
    PioStringView value_kind;
} PioJsonObjectEntryView;

/**
 * Shape and conventions of one prepared DC OPF calculation.
 */
typedef struct {
    PioStringView name;
    size_t bus_count;
    size_t generator_count;
    size_t branch_count;
    size_t source_generator_count;
    size_t source_branch_count;
    double base_mva;
    PioStringView units;
    PioStringView branch_susceptance_formula;
    PioStringView objective;
    bool skip_zero_impedance;
    bool synthesize_unrated_limits;
    bool correct_angle_difference_bounds;
    size_t reference_bus_count;
    size_t skipped_zero_impedance_count;
} PioDcOpfPreparationView;

/**
 * Borrowed `size_t` values.
 */
typedef struct {
    const size_t *data;
    size_t len;
} PioSizeView;

/**
 * One dense bus row in a DC OPF preparation.
 */
typedef struct {
    size_t bus_id;
    size_t analysis_row;
    size_t source_row;
    bool has_source_row;
    double active_power_demand;
    double shunt_conductance;
    double phase_shift_injection;
} PioDcOpfBusView;

/**
 * Borrowed `double` values.
 */
typedef struct {
    const double *data;
    size_t len;
} PioF64View;

/**
 * One generator row in a DC OPF preparation.
 */
typedef struct {
    PioStringView component_id;
    size_t bus_index;
    size_t analysis_row;
    size_t source_row;
    bool has_source_row;
    double quadratic_cost;
    double linear_cost;
    double constant_cost;
    bool has_piecewise_linear_cost;
    PioF64View piecewise_linear_power;
    PioF64View piecewise_linear_value;
    double active_power_max;
    double active_power_min;
    bool capability_active;
} PioDcOpfGeneratorView;

/**
 * One active branch row in a DC OPF preparation.
 */
typedef struct {
    PioStringView component_id;
    size_t from_bus_index;
    size_t to_bus_index;
    double susceptance_magnitude;
    double phase_shift_radians;
    double active_power_max;
    double angle_difference_min_radians;
    double angle_difference_max_radians;
    size_t analysis_row;
    PioStringView source_kind;
    size_t source_row;
    size_t winding;
    bool has_winding;
    bool thermal_limit_active;
    bool angle_bound_active;
} PioDcOpfBranchView;

/**
 * Shape and conventions of one prepared AC OPF calculation.
 */
typedef struct {
    PioStringView name;
    size_t bus_count;
    size_t generator_count;
    size_t storage_count;
    size_t branch_count;
    size_t source_generator_count;
    size_t source_branch_count;
    double base_mva;
    PioStringView units;
    PioStringView objective;
    bool skip_zero_impedance;
    bool synthesize_unrated_limits;
    bool correct_angle_difference_bounds;
    size_t reference_bus_count;
    size_t skipped_zero_impedance_count;
} PioAcOpfPreparationView;

/**
 * One dense bus row in an AC OPF preparation.
 */
typedef struct {
    size_t bus_id;
    size_t analysis_row;
    size_t source_row;
    bool has_source_row;
    double active_power_demand;
    double reactive_power_demand;
    double shunt_conductance;
    double shunt_susceptance;
    double voltage_magnitude_min_pu;
    double voltage_magnitude_max_pu;
    double initial_voltage_magnitude_pu;
    double initial_voltage_angle_radians;
    bool voltage_bound_active;
} PioAcOpfBusView;

/**
 * One generator row in an AC OPF preparation.
 */
typedef struct {
    PioStringView component_id;
    size_t bus_index;
    size_t analysis_row;
    size_t source_row;
    bool has_source_row;
    double quadratic_cost;
    double linear_cost;
    double constant_cost;
    bool has_piecewise_linear_cost;
    PioF64View piecewise_linear_power;
    PioF64View piecewise_linear_value;
    double active_power_max;
    double active_power_min;
    double reactive_power_max;
    double reactive_power_min;
    double initial_active_power;
    double initial_reactive_power;
    double voltage_magnitude_setpoint_pu;
    bool capability_active;
} PioAcOpfGeneratorView;

/**
 * One storage row in an AC OPF preparation.
 */
typedef struct {
    PioStringView component_id;
    size_t bus_index;
    size_t source_row;
    double initial_active_power;
    double initial_reactive_power;
    double energy;
    double energy_rating;
    double charge_rating;
    double discharge_rating;
    double charge_efficiency;
    double discharge_efficiency;
    double apparent_power_max;
    double reactive_power_min;
    double reactive_power_max;
    double resistance_pu;
    double reactance_pu;
    double active_power_loss;
    double reactive_power_loss;
    bool in_service;
} PioAcOpfStorageView;

/**
 * One active branch row in an AC OPF preparation.
 */
typedef struct {
    PioStringView component_id;
    size_t from_bus_index;
    size_t to_bus_index;
    double series_conductance;
    double series_susceptance;
    double from_conductance;
    double from_susceptance;
    double to_conductance;
    double to_susceptance;
    double tap_ratio;
    double phase_shift_radians;
    double apparent_power_max;
    double angle_difference_min_radians;
    double angle_difference_max_radians;
    size_t analysis_row;
    PioStringView source_kind;
    size_t source_row;
    size_t winding;
    bool has_winding;
    bool thermal_limit_active;
    bool angle_bound_active;
} PioAcOpfBranchView;

/**
 * One bus boundary specification in a DC power flow instance.
 */
typedef struct {
    size_t bus_id;
    PioStringView kind;
    double net_active_power_mw;
    double voltage_angle_degrees;
} PioDcBusSpecificationView;

/**
 * One bus boundary specification in an AC power flow instance.
 */
typedef struct {
    size_t bus_id;
    PioStringView kind;
    double net_active_power_mw;
    double net_reactive_power_mvar;
    double voltage_magnitude_pu;
    double voltage_angle_degrees;
} PioAcBusSpecificationView;

/**
 * One typed objective term.
 */
typedef struct {
    PioStringView kind;
} PioObjectiveTermView;

/**
 * One active constraint family and its element selection.
 */
typedef struct {
    PioStringView family;
    PioStringView selection;
    size_t identity_count;
} PioActiveConstraintView;

/**
 * One prescribed multiconductor load terminal power.
 */
typedef struct {
    PioStringView load;
    size_t terminal_count;
    PioStringView voltage_model;
} PioPrescribedTerminalPowerView;

/**
 * One terminal of a prescribed multiconductor load.
 */
typedef struct {
    PioStringView terminal;
    double active_power_w;
    double reactive_power_var;
    double nominal_voltage_v;
    bool has_nominal_voltage;
    double active_impedance_fraction;
    double active_current_fraction;
    double active_power_fraction;
    double reactive_impedance_fraction;
    double reactive_current_fraction;
    double reactive_power_fraction;
    double active_power_exponent;
    double reactive_power_exponent;
} PioTerminalPowerView;

/**
 * One prescribed multiconductor source terminal voltage.
 */
typedef struct {
    PioStringView source;
    size_t terminal_count;
} PioPrescribedSourceVoltageView;

/**
 * One terminal of a prescribed multiconductor source.
 */
typedef struct {
    PioStringView terminal;
    double magnitude_v;
    double angle_radians;
} PioTerminalVoltageView;

/**
 * One isolated multiconductor terminal.
 */
typedef struct {
    PioStringView bus;
    PioStringView terminal;
} PioIsolatedTerminalView;

/**
 * One active multiconductor equipment control.
 */
typedef struct {
    PioStringView kind;
    PioStringView component_id;
} PioActiveControlView;

/**
 * Set sizes and time horizon of one AC SCUC instance.
 */
typedef struct {
    size_t period_count;
    size_t device_count;
    size_t producer_count;
    size_t consumer_count;
    size_t shunt_count;
    size_t branch_switching_cost_count;
    size_t transformer_control_count;
    size_t active_reserve_zone_count;
    size_t reactive_reserve_zone_count;
    size_t contingency_count;
} PioScucDimensionsView;

/**
 * Required SCUC violation costs.
 */
typedef struct {
    double active_power_balance;
    double reactive_power_balance;
    double branch_thermal_limit;
    double energy_requirement;
} PioScucViolationCostView;

/**
 * Borrowed stable component identity.
 */
typedef struct {
    PioStringView component_type;
    PioStringView local_id;
} PioComponentIdView;

/**
 * Active power ramp limits for one SCUC device, in per unit per hour.
 */
typedef struct {
    double up_pu_per_hour;
    double down_pu_per_hour;
    double startup_pu_per_hour;
    double shutdown_pu_per_hour;
} PioScucRampLimitsView;

/**
 * Reserve quantity limits for one SCUC device.
 */
typedef struct {
    double regulation_up_pu;
    double regulation_down_pu;
    double synchronized_pu;
    double nonsynchronized_pu;
    double ramping_up_online_pu;
    double ramping_down_online_pu;
    double ramping_up_offline_pu;
    double ramping_down_offline_pu;
} PioScucReserveLimitsView;

/**
 * Initial commitment durations for one SCUC device.
 */
typedef struct {
    double accumulated_up_time_hours;
    double accumulated_down_time_hours;
} PioScucInitialCommitmentView;

/**
 * Additional active and reactive power capability relation for one device.
 */
typedef struct {
    PioStringView kind;
    double reactive_power_at_zero_active_power_pu;
    double reactive_power_at_zero_active_power_min_pu;
    double reactive_power_at_zero_active_power_max_pu;
    double slope;
    double slope_min;
    double slope_max;
} PioScucReactiveCapabilityView;

/**
 * One SCUC producer or consumer.
 */
typedef struct {
    PioComponentIdView id;
    PioStringView kind;
    bool initial_on_status;
    double on_cost;
    double startup_cost;
    double shutdown_cost;
    double minimum_up_time_hours;
    double minimum_down_time_hours;
    PioScucRampLimitsView ramp_limits;
    PioScucReserveLimitsView reserve_limits;
    PioScucInitialCommitmentView initial_commitment;
    PioScucReactiveCapabilityView reactive_capability;
    size_t period_count;
    size_t startup_cost_adjustment_count;
    size_t startup_limit_count;
    size_t energy_upper_bound_count;
    size_t energy_lower_bound_count;
} PioScucDeviceView;

/**
 * One downtime dependent startup cost adjustment.
 */
typedef struct {
    double cost;
    double maximum_down_time_hours;
} PioScucStartupCostAdjustmentView;

/**
 * One limit on the number of device startups during a time window.
 */
typedef struct {
    double start_time_hours;
    double end_time_hours;
    uint64_t maximum_startups;
} PioScucStartupLimitView;

/**
 * One energy requirement over a time window, in per unit as defined by GOC3.
 */
typedef struct {
    double start_time_hours;
    double end_time_hours;
    double energy_pu;
} PioScucEnergyRequirementView;

/**
 * Reserve costs for one device and one interval, in $/(p.u. h).
 */
typedef struct {
    double regulation_up;
    double regulation_down;
    double synchronized;
    double nonsynchronized;
    double ramping_up_online;
    double ramping_down_online;
    double ramping_up_offline;
    double ramping_down_offline;
    double reactive_up;
    double reactive_down;
} PioScucReserveCostsView;

/**
 * One SCUC device period.
 */
typedef struct {
    bool on_status_min;
    bool on_status_max;
    double active_power_min_pu;
    double active_power_max_pu;
    double reactive_power_min_pu;
    double reactive_power_max_pu;
    size_t energy_cost_block_count;
    PioScucReserveCostsView reserve_costs;
} PioScucDevicePeriodView;

/**
 * One piecewise linear active energy cost block.
 */
typedef struct {
    double marginal_cost;
    double block_size_pu;
} PioScucEnergyCostBlockView;

/**
 * Discrete step limits for one SCUC shunt.
 */
typedef struct {
    PioComponentIdView id;
    double conductance_per_step_pu;
    double susceptance_per_step_pu;
    int64_t step_min;
    int64_t step_max;
    int64_t initial_step;
} PioScucShuntView;

/**
 * Connection and disconnection costs for one switchable AC branch.
 */
typedef struct {
    PioComponentIdView id;
    double connection_cost;
    double disconnection_cost;
} PioScucBranchSwitchingCostView;

/**
 * Tap ratio and phase shift bounds for one transformer.
 */
typedef struct {
    PioComponentIdView id;
    double tap_ratio_min;
    double tap_ratio_max;
    double phase_shift_min_radians;
    double phase_shift_max_radians;
} PioScucTransformerControlView;

/**
 * One active power reserve zone.
 */
typedef struct {
    PioComponentIdView id;
    double regulation_up_requirement_fraction;
    double regulation_down_requirement_fraction;
    double synchronized_requirement_fraction;
    double nonsynchronized_requirement_fraction;
    double regulation_up_violation_cost;
    double regulation_down_violation_cost;
    double synchronized_violation_cost;
    double nonsynchronized_violation_cost;
    double ramping_up_violation_cost;
    double ramping_down_violation_cost;
    size_t period_count;
    size_t bus_count;
} PioScucActiveReserveZoneView;

/**
 * One period of an active power reserve zone.
 */
typedef struct {
    double ramping_up_requirement_pu;
    double ramping_down_requirement_pu;
} PioScucActiveReservePeriodView;

/**
 * One reactive power reserve zone.
 */
typedef struct {
    PioComponentIdView id;
    double reactive_up_violation_cost;
    double reactive_down_violation_cost;
    size_t period_count;
    size_t bus_count;
} PioScucReactiveReserveZoneView;

/**
 * One period of a reactive power reserve zone.
 */
typedef struct {
    double reactive_up_requirement_pu;
    double reactive_down_requirement_pu;
} PioScucReactiveReservePeriodView;

/**
 * One named SCUC contingency.
 */
typedef struct {
    PioComponentIdView id;
    size_t component_count;
} PioScucContingencyView;

/**
 * One component removed by a SCUC contingency.
 */
typedef struct {
    PioComponentIdView id;
} PioScucContingencyComponentView;

/**
 * Coordinate metadata for a balanced network.
 */
typedef struct {
    bool has_geo;
    PioStringView space;
    PioStringView crs;
    bool has_crs;
    PioStringView kind;
    bool has_kind;
    bool has_canvas;
    double canvas_width;
    bool has_canvas_width;
    double canvas_height;
    bool has_canvas_height;
    PioStringView canvas_units;
    bool has_canvas_units;
} PioBalancedGeoView;

/**
 * Exact table lengths in source neutral detailed connectivity.
 */
typedef struct {
    size_t omitted_fields;
    size_t component_metadata;
    size_t subnetworks;
    size_t substations;
    size_t voltage_levels;
    size_t bus_breaker_buses;
    size_t calculated_buses;
    size_t connectivity_nodes;
    size_t busbar_sections;
    size_t junctions;
    size_t terminals;
    size_t switches;
    size_t internal_connections;
    size_t operational_limit_groups;
    size_t tap_changers;
    size_t equipment_reactive_limits;
    size_t boundary_lines;
    size_t tie_lines;
    size_t dc_converter_units;
    size_t dc_topological_nodes;
    size_t dc_nodes;
    size_t dc_grounds;
    size_t dc_busbars;
    size_t dc_lines;
    size_t dc_series_devices;
    size_t dc_switches;
    size_t voltage_source_converters;
    size_t line_commutated_converters;
} PioDetailedConnectivityCountsView;

/**
 * One source field that was absent rather than explicitly assigned a value.
 */
typedef struct {
    PioComponentIdView component;
    PioStringView field;
} PioOmittedFieldView;

/**
 * Source neutral metadata attached to one stable component identity.
 */
typedef struct {
    PioComponentIdView component;
    PioStringView name;
    bool has_name;
    PioComponentIdView equipment_container;
    bool has_equipment_container;
    bool fictitious;
    size_t alias_count;
    size_t external_identifier_count;
    size_t property_count;
} PioComponentMetadataView;

/**
 * One source neutral component alias.
 */
typedef struct {
    PioStringView value;
    PioStringView alias_type;
    bool has_alias_type;
} PioComponentAliasView;

/**
 * One source neutral external component identifier.
 */
typedef struct {
    PioStringView value;
    PioStringView authority;
    bool has_authority;
} PioExternalIdentifierView;

/**
 * One source neutral string property.
 */
typedef struct {
    PioStringView name;
    PioStringView value;
} PioStringPropertyView;

/**
 * Source neutral case metadata attached to one subnetwork.
 */
typedef struct {
    PioStringView case_date;
    bool has_case_date;
    int32_t forecast_distance;
    bool has_forecast_distance;
    PioStringView source_model_format;
    bool has_source_model_format;
    PioStringView minimum_validation_level;
    bool has_minimum_validation_level;
} PioCaseMetadataView;

/**
 * One PowSybl subnetwork contained directly by the balanced network.
 */
typedef struct {
    PioComponentIdView component;
    PioComponentIdView parent;
    PioCaseMetadataView case_metadata;
    size_t component_count;
} PioSubnetworkView;

/**
 * One source neutral substation.
 */
typedef struct {
    PioComponentIdView component;
    PioStringView country;
    bool has_country;
    PioStringView operator_name;
    bool has_operator_name;
    size_t geographical_tag_count;
} PioSubstationView;

/**
 * One source neutral voltage level.
 */
typedef struct {
    PioComponentIdView component;
    PioComponentIdView substation;
    bool has_substation;
    double nominal_voltage_kv;
    double low_voltage_limit_kv;
    bool has_low_voltage_limit;
    double high_voltage_limit_kv;
    bool has_high_voltage_limit;
    PioStringView topology_kind;
    size_t bus_count;
} PioVoltageLevelView;

/**
 * One configured bus in bus breaker topology.
 */
typedef struct {
    PioComponentIdView component;
    PioComponentIdView voltage_level;
    size_t calculated_bus_id;
    bool has_calculated_bus;
    double voltage_kv;
    bool has_voltage;
    double angle_degrees;
    bool has_angle;
} PioBusBreakerBusView;

/**
 * One calculated bus explicitly recorded in node breaker topology.
 */
typedef struct {
    PioComponentIdView voltage_level;
    size_t calculated_bus_id;
    size_t node_count;
    double voltage_kv;
    bool has_voltage;
    double angle_degrees;
    bool has_angle;
} PioCalculatedBusView;

/**
 * One source neutral connectivity node.
 */
typedef struct {
    PioComponentIdView component;
    PioComponentIdView voltage_level;
    int32_t node_number;
    bool has_node_number;
    size_t calculated_bus_id;
    bool has_calculated_bus;
} PioConnectivityNodeView;

/**
 * One source neutral busbar section.
 */
typedef struct {
    PioComponentIdView component;
    PioComponentIdView voltage_level;
    PioComponentIdView node;
} PioBusbarSectionView;

/**
 * One source neutral CIM junction.
 */
typedef struct {
    PioComponentIdView component;
} PioJunctionView;

/**
 * One source neutral AC terminal.
 */
typedef struct {
    PioComponentIdView component;
    bool has_component;
    PioComponentIdView equipment;
    uint8_t terminal;
    PioComponentIdView voltage_level;
    PioComponentIdView bus;
    bool has_bus;
    PioComponentIdView connectable_bus;
    bool has_connectable_bus;
    PioComponentIdView node;
    bool has_node;
    bool connected;
    double active_power_mw;
    bool has_active_power;
    double reactive_power_mvar;
    bool has_reactive_power;
} PioDetailedTerminalView;

/**
 * One source neutral bus breaker or node breaker switch.
 */
typedef struct {
    PioComponentIdView component;
    PioComponentIdView voltage_level;
    PioStringView kind;
    PioStringView endpoint1_kind;
    PioComponentIdView endpoint1;
    PioStringView endpoint2_kind;
    PioComponentIdView endpoint2;
    bool open;
    bool retained;
} PioTopologySwitchView;

/**
 * One permanent connection between two node breaker connectivity nodes.
 */
typedef struct {
    PioComponentIdView voltage_level;
    PioComponentIdView node1;
    PioComponentIdView node2;
} PioInternalConnectionView;

/**
 * One named source neutral loading limit set at an equipment terminal.
 */
typedef struct {
    PioComponentIdView equipment;
    uint8_t terminal;
    PioStringView id;
    bool selected;
    size_t property_count;
    bool has_current_limits;
    double current_permanent_limit_a;
    PioStringView current_permanent_limit_name;
    bool has_current_permanent_limit;
    bool has_current_permanent_limit_name;
    size_t current_temporary_limit_count;
    bool has_active_power_limits;
    double active_power_permanent_limit_mw;
    PioStringView active_power_permanent_limit_name;
    bool has_active_power_permanent_limit;
    bool has_active_power_permanent_limit_name;
    size_t active_power_temporary_limit_count;
    bool has_apparent_power_limits;
    double apparent_power_permanent_limit_mva;
    PioStringView apparent_power_permanent_limit_name;
    bool has_apparent_power_permanent_limit;
    bool has_apparent_power_permanent_limit_name;
    size_t apparent_power_temporary_limit_count;
} PioOperationalLimitGroupView;

/**
 * One temporary source neutral loading limit.
 */
typedef struct {
    PioStringView name;
    double value;
    uint64_t acceptable_duration_seconds;
    bool fictitious;
} PioTemporaryLimitView;

/**
 * Min/max or active power dependent reactive limits.
 */
typedef struct {
    PioStringView kind;
    double minimum_reactive_power_mvar;
    double maximum_reactive_power_mvar;
    bool has_minimum_and_maximum;
    PioStringView curve_style;
    bool has_curve_style;
    size_t property_count;
    size_t point_count;
} PioReactiveLimitsView;

/**
 * Optional generation attached to one PowSybl boundary line.
 */
typedef struct {
    bool voltage_regulation_on;
    double minimum_active_power_mw;
    bool has_minimum_active_power;
    double maximum_active_power_mw;
    bool has_maximum_active_power;
    double target_active_power_mw;
    bool has_target_active_power;
    double target_reactive_power_mvar;
    bool has_target_reactive_power;
    double target_voltage_kv;
    bool has_target_voltage;
    PioReactiveLimitsView reactive_limits;
    bool has_reactive_limits;
} PioBoundaryLineGenerationView;

/**
 * One PowSybl boundary line retained beside the balanced calculation view.
 */
typedef struct {
    PioComponentIdView component;
    PioComponentIdView voltage_level;
    double active_power_setpoint_mw;
    double reactive_power_setpoint_mvar;
    double resistance_ohm;
    double reactance_ohm;
    double conductance_siemens;
    double susceptance_siemens;
    PioStringView pairing_key;
    bool has_pairing_key;
    PioBoundaryLineGenerationView generation;
    bool has_generation;
    PioComponentIdView calculation_load;
    bool has_calculation_load;
    PioComponentIdView calculation_generator;
    bool has_calculation_generator;
} PioBoundaryLineView;

/**
 * One point of a reactive capability curve.
 */
typedef struct {
    double active_power_mw;
    double minimum_reactive_power_mvar;
    double maximum_reactive_power_mvar;
    size_t property_count;
} PioReactiveCapabilityCurvePointView;

/**
 * One PowSybl tie line and the two boundary lines that define it.
 */
typedef struct {
    PioComponentIdView component;
    PioComponentIdView boundary_line1;
    PioComponentIdView boundary_line2;
    PioComponentIdView calculation_branch;
    bool has_calculation_branch;
} PioTieLineView;

/**
 * Borrowed reference to one numbered equipment terminal.
 */
typedef struct {
    PioComponentIdView equipment;
    uint8_t terminal;
} PioTerminalReferenceView;

/**
 * One source neutral transformer tap changer.
 */
typedef struct {
    PioComponentIdView component;
    bool has_component;
    PioComponentIdView transformer;
    uint8_t winding;
    PioStringView kind;
    int32_t tap_position;
    bool has_tap_position;
    int32_t solved_tap_position;
    bool has_solved_tap_position;
    int32_t low_tap_position;
    int32_t neutral_tap_position;
    bool has_neutral_tap_position;
    int32_t normal_tap_position;
    bool has_normal_tap_position;
    double voltage_step_increment_percent;
    bool has_voltage_step_increment_percent;
    bool load_tap_changing_capabilities;
    bool regulating;
    PioStringView regulation_mode;
    bool has_regulation_mode;
    double regulation_value;
    bool has_regulation_value;
    double target_deadband;
    bool has_target_deadband;
    PioTerminalReferenceView regulation_terminal;
    bool has_regulation_terminal;
    size_t step_count;
} PioTapChangerView;

/**
 * One source neutral transformer tap changer step.
 */
typedef struct {
    int32_t position;
    double ratio_pu;
    double phase_shift_degrees;
    double resistance_deviation_percent;
    double reactance_deviation_percent;
    double conductance_deviation_percent;
    double susceptance_deviation_percent;
} PioTapChangerStepView;

/**
 * Reactive limits retained for one equipment record.
 */
typedef struct {
    PioComponentIdView equipment;
    PioReactiveLimitsView limits;
} PioEquipmentReactiveLimitsView;

/**
 * One source neutral DC converter unit.
 */
typedef struct {
    PioComponentIdView component;
    PioComponentIdView substation;
    bool has_substation;
    PioStringView operation_mode;
} PioDcConverterUnitView;

/**
 * One physical or energized node in source neutral DC connectivity.
 */
typedef struct {
    PioComponentIdView component;
    PioStringView kind;
    double nominal_voltage_kv;
    bool has_nominal_voltage;
    double voltage_kv;
    bool has_voltage;
    PioComponentIdView dc_converter_unit;
    bool has_dc_converter_unit;
    PioComponentIdView dc_topological_node;
    bool has_dc_topological_node;
} PioDcNodeView;

/**
 * One terminal of source neutral DC conducting equipment.
 */
typedef struct {
    PioComponentIdView component;
    bool has_component;
    uint32_t sequence_number;
    bool has_sequence_number;
    PioComponentIdView dc_node;
    bool has_dc_node;
    PioComponentIdView dc_topological_node;
    bool has_dc_topological_node;
    PioStringView polarity;
    bool has_polarity;
    bool connected;
    bool has_connected;
    double active_power_mw;
    bool has_active_power;
    double current_a;
    bool has_current;
} PioDcTerminalView;

/**
 * One source neutral DC conducting equipment record.
 */
typedef struct {
    PioComponentIdView component;
    PioComponentIdView equipment_container;
    bool has_equipment_container;
    PioStringView kind;
    size_t terminal_count;
    PioDcTerminalView terminal1;
    PioDcTerminalView terminal2;
    double rated_dc_voltage_kv;
    bool has_rated_dc_voltage;
    double resistance_ohm;
    bool has_resistance;
    double inductance_h;
    bool has_inductance;
    double capacitance_f;
    bool has_capacitance;
    double length_km;
    bool has_length;
    PioStringView switch_kind;
    bool has_switch_kind;
    bool open;
    bool has_open;
} PioDcEquipmentView;

/**
 * One source neutral AC/DC converter.
 */
typedef struct {
    PioComponentIdView component;
    PioStringView kind;
    PioComponentIdView dc_converter_unit;
    bool has_dc_converter_unit;
    PioDcTerminalView dc_terminal1;
    PioDcTerminalView dc_terminal2;
    double base_apparent_power_mva;
    bool has_base_apparent_power;
    double minimum_active_power_mw;
    bool has_minimum_active_power;
    double maximum_active_power_mw;
    bool has_maximum_active_power;
    double minimum_dc_voltage_kv;
    bool has_minimum_dc_voltage;
    double maximum_dc_voltage_kv;
    bool has_maximum_dc_voltage;
    double rated_dc_voltage_kv;
    bool has_rated_dc_voltage;
    double valve_u0_kv;
    bool has_valve_u0;
    uint32_t number_of_valves;
    bool has_number_of_valves;
    double idle_loss_mw;
    bool has_idle_loss;
    double switching_loss_mw_per_ampere;
    bool has_switching_loss;
    double resistive_loss_ohm;
    bool has_resistive_loss;
    PioStringView control_mode;
    bool has_control_mode;
    double active_power_at_pcc_mw;
    bool has_active_power_at_pcc;
    double reactive_power_at_pcc_mvar;
    bool has_reactive_power_at_pcc;
    double target_active_power_mw;
    bool has_target_active_power;
    double target_dc_voltage_kv;
    bool has_target_dc_voltage;
    PioTerminalReferenceView pcc_terminal;
    bool has_pcc_terminal;
    size_t droop_curve_segment_count;
    bool has_droop_curve;
    double droop;
    bool has_droop;
    double droop_compensation;
    bool has_droop_compensation;
    double q_share;
    bool has_q_share;
    double maximum_modulation_index;
    bool has_maximum_modulation_index;
    double maximum_valve_current_a;
    bool has_maximum_valve_current;
    double dc_current_a;
    bool has_dc_current;
    double ac_voltage_kv;
    bool has_ac_voltage;
    double dc_voltage_kv;
    bool has_dc_voltage;
    bool voltage_regulator_on;
    bool has_voltage_regulator_on;
    double voltage_setpoint_kv;
    bool has_voltage_setpoint;
    double reactive_power_setpoint_mvar;
    bool has_reactive_power_setpoint;
    PioReactiveLimitsView reactive_limits;
    bool has_reactive_limits;
    double pole_loss_active_power_mw;
    bool has_pole_loss_active_power;
    PioStringView reactive_model;
    bool has_reactive_model;
    double power_factor;
    bool has_power_factor;
    PioStringView operating_mode;
    bool has_operating_mode;
    double rated_dc_current_a;
    bool has_rated_dc_current;
    double minimum_alpha_degrees;
    bool has_minimum_alpha;
    double maximum_alpha_degrees;
    bool has_maximum_alpha;
    double minimum_gamma_degrees;
    bool has_minimum_gamma;
    double maximum_gamma_degrees;
    bool has_maximum_gamma;
    double target_alpha_degrees;
    bool has_target_alpha;
    double target_gamma_degrees;
    bool has_target_gamma;
    double target_dc_current_a;
    bool has_target_dc_current;
    double alpha_degrees;
    bool has_alpha;
    double gamma_degrees;
    bool has_gamma;
    double delta_degrees;
    bool has_delta;
    double uf_kv;
    bool has_uf;
    double uv_kv;
    bool has_uv;
} PioAcDcConverterView;

/**
 * One segment of an AC/DC converter DC voltage droop curve.
 */
typedef struct {
    double minimum_voltage_kv;
    double maximum_voltage_kv;
    double k;
} PioDroopCurveSegmentView;

/**
 * One point in a balanced network coordinate space.
 */
typedef struct {
    double x;
    double y;
    PioStringView kind;
    bool has_kind;
} PioBalancedLocationView;

/**
 * One balanced bus. String and coefficient spans borrow from the network.
 */
typedef struct {
    PioStringView component_id;
    bool has_component_id;
    size_t id;
    PioStringView bus_type;
    double vm_pu;
    double va_degrees;
    double base_kv;
    double vmax_pu;
    double vmin_pu;
    bool has_emergency_voltage_limits;
    double emergency_vmax_pu;
    double emergency_vmin_pu;
    size_t area;
    size_t zone;
    PioStringView name;
    bool has_name;
    PioBalancedLocationView location;
    bool has_location;
} PioBalancedBusView;

/**
 * Voltage dependence attached to one balanced load.
 */
typedef struct {
    PioStringView kind;
    double p_constant_power_mw;
    double q_constant_power_mvar;
    double p_constant_current_mw;
    double q_constant_current_mvar;
    double p_constant_impedance_mw;
    double q_constant_impedance_mvar;
    double exponential_p_mw;
    double exponential_q_mvar;
    double gamma_p;
    double gamma_q;
    double nominal_voltage_pu;
    bool has_nominal_voltage;
    int32_t load_type;
    bool has_load_type;
    double scaling;
    bool has_scaling;
} PioBalancedLoadVoltageModelView;

/**
 * One balanced load.
 */
typedef struct {
    PioStringView component_id;
    bool has_component_id;
    size_t bus_id;
    double p_mw;
    double q_mvar;
    bool in_service;
    PioBalancedLoadVoltageModelView voltage_model;
} PioBalancedLoadView;

/**
 * One balanced shunt.
 */
typedef struct {
    PioStringView component_id;
    bool has_component_id;
    size_t bus_id;
    double conductance_mw;
    double susceptance_mvar;
    bool in_service;
    uint32_t section_count;
    bool has_section_count;
    bool has_control;
    PioStringView control_mode;
    double control_vmax_pu;
    double control_vmin_pu;
    size_t control_bus_id;
    bool has_control_bus;
    double control_reactive_range_percent;
    size_t control_block_count;
} PioBalancedShuntView;

/**
 * One switched shunt block.
 */
typedef struct {
    uint32_t steps;
    double conductance_mw;
    double susceptance_mvar;
} PioShuntBlockView;

/**
 * One balanced static VAR compensator.
 */
typedef struct {
    PioStringView component_id;
    bool has_component_id;
    size_t bus_id;
    double minimum_susceptance_siemens;
    double maximum_susceptance_siemens;
    double voltage_setpoint_kv;
    double reactive_power_setpoint_mvar;
    PioStringView regulation_mode;
    bool regulating;
    PioTerminalReferenceView regulating_terminal;
    bool has_regulating_terminal;
    double active_power_mw;
    double reactive_power_mvar;
    bool in_service;
} PioBalancedStaticVarCompensatorView;

/**
 * Automatic transformer tap or phase control.
 */
typedef struct {
    PioStringView mode;
    bool enabled;
    size_t controlled_bus_id;
    bool has_controlled_bus;
    bool controlled_bus_on_winding_side;
    PioTerminalReferenceView regulating_terminal;
    bool has_regulating_terminal;
    double tap_min;
    double tap_max;
    double band_min;
    double band_max;
    uint32_t tap_position_count;
    double mva_base;
    double winding_connection_angle;
    bool has_winding_connection_angle;
} PioTransformerControlView;

/**
 * One balanced branch or two winding transformer.
 */
typedef struct {
    PioStringView component_id;
    bool has_component_id;
    PioStringView name;
    bool has_name;
    size_t from_bus_id;
    size_t to_bus_id;
    double resistance_pu;
    double reactance_pu;
    double total_charging_susceptance_pu;
    bool terminal_charging_is_explicit;
    double from_conductance_pu;
    double from_susceptance_pu;
    double to_conductance_pu;
    double to_susceptance_pu;
    double rate_a_mva;
    double rate_b_mva;
    double rate_c_mva;
    size_t additional_rating_count;
    bool has_current_ratings;
    double current_rating_a;
    double current_rating_b;
    double current_rating_c;
    double tap_ratio;
    double effective_tap_ratio;
    double phase_shift_degrees;
    bool in_service;
    double angle_min_degrees;
    double angle_max_degrees;
    PioTransformerControlView control;
    bool has_control;
    size_t route_point_count;
    bool has_route;
} PioBalancedBranchView;

/**
 * One named branch MVA rating beyond rating A, B, and C.
 */
typedef struct {
    PioStringView name;
    double rate_mva;
} PioBranchRatingView;

/**
 * One generator cost curve.
 */
typedef struct {
    uint8_t model;
    double startup;
    double shutdown;
    size_t ncost;
    PioF64View coefficients;
} PioGeneratorCostView;

/**
 * Governor and distributed slack settings for a generator or storage element.
 */
typedef struct {
    bool participate;
    double droop_percent;
    bool has_droop_percent;
    double participation_factor;
    bool has_participation_factor;
    double minimum_target_active_power_mw;
    bool has_minimum_target_active_power;
    double maximum_target_active_power_mw;
    bool has_maximum_target_active_power;
} PioActivePowerControlView;

/**
 * One balanced generator.
 */
typedef struct {
    PioStringView component_id;
    bool has_component_id;
    size_t bus_id;
    PioStringView energy_source;
    double active_power_mw;
    double reactive_power_mvar;
    double active_power_max_mw;
    double active_power_min_mw;
    double reactive_power_max_mvar;
    double reactive_power_min_mvar;
    double voltage_setpoint_pu;
    double machine_base_mva;
    bool in_service;
    bool has_cost;
    PioGeneratorCostView cost;
    size_t regulated_bus_id;
    bool has_regulated_bus;
    size_t capability_count;
    PioActivePowerControlView active_power_control;
    bool has_active_power_control;
    bool voltage_regulation_on;
    PioTerminalReferenceView regulating_terminal;
    bool has_regulating_terminal;
} PioBalancedGeneratorView;

/**
 * One optional generator capability or ramp field.
 */
typedef struct {
    PioStringView name;
    double value;
    bool has_value;
} PioGeneratorCapabilityView;

/**
 * One balanced storage element.
 */
typedef struct {
    PioStringView component_id;
    bool has_component_id;
    size_t bus_id;
    double active_power_mw;
    double reactive_power_mvar;
    double energy_mwh;
    double energy_rating_mwh;
    double charge_rating_mw;
    double discharge_rating_mw;
    double charge_efficiency;
    double discharge_efficiency;
    double thermal_rating_mva;
    double current_rating;
    bool has_current_rating;
    double reactive_power_min_mvar;
    double reactive_power_max_mvar;
    double resistance_pu;
    double reactance_pu;
    double active_power_loss_mw;
    double reactive_power_loss_mvar;
    bool in_service;
    PioActivePowerControlView active_power_control;
    bool has_active_power_control;
} PioBalancedStorageView;

/**
 * One balanced transmission switch.
 */
typedef struct {
    PioStringView component_id;
    bool has_component_id;
    size_t from_bus_id;
    size_t to_bus_id;
    bool closed;
    double thermal_rating_mva;
    bool has_thermal_rating;
    double current_rating_a;
    bool has_current_rating;
    double from_active_power_mw;
    bool has_from_active_power;
    double from_reactive_power_mvar;
    bool has_from_reactive_power;
    double to_active_power_mw;
    bool has_to_active_power;
    double to_reactive_power_mvar;
    bool has_to_reactive_power;
} PioBalancedSwitchView;

/**
 * One AC terminal converter station of a balanced HVDC line.
 */
typedef struct {
    PioComponentIdView component;
    PioStringView kind;
    double loss_factor_percent;
    bool voltage_regulator_on;
    bool has_voltage_regulator_on;
    double voltage_setpoint_kv;
    bool has_voltage_setpoint;
    double reactive_power_setpoint_mvar;
    bool has_reactive_power_setpoint;
    double power_factor;
    bool has_power_factor;
    PioTerminalReferenceView regulating_terminal;
    bool has_regulating_terminal;
} PioBalancedHvdcConverterView;

/**
 * One balanced two terminal HVDC line.
 */
typedef struct {
    PioStringView component_id;
    bool has_component_id;
    size_t from_bus_id;
    size_t to_bus_id;
    bool in_service;
    double from_active_power_mw;
    double to_active_power_mw;
    double from_reactive_power_mvar;
    double to_reactive_power_mvar;
    double from_voltage_pu;
    double to_voltage_pu;
    double minimum_active_power_mw;
    double maximum_active_power_mw;
    double minimum_from_reactive_power_mvar;
    double maximum_from_reactive_power_mvar;
    double minimum_to_reactive_power_mvar;
    double maximum_to_reactive_power_mvar;
    double constant_loss_mw;
    double proportional_loss;
    double resistance_ohm;
    bool has_resistance;
    double nominal_voltage_kv;
    bool has_nominal_voltage;
    PioStringView converters_mode;
    bool has_converters_mode;
    PioBalancedHvdcConverterView converter1;
    bool has_converter1;
    PioBalancedHvdcConverterView converter2;
    bool has_converter2;
    PioGeneratorCostView cost;
    bool has_cost;
} PioBalancedHvdcView;

/**
 * One balanced three winding transformer.
 */
typedef struct {
    PioStringView component_id;
    bool has_component_id;
    PioStringView name;
    bool has_name;
    size_t winding_count;
    size_t impedance_count;
    double star_voltage_magnitude_pu;
    double star_voltage_angle_degrees;
    double magnetizing_conductance_pu;
    double magnetizing_susceptance_pu;
    bool in_service;
} PioBalancedThreeWindingTransformerView;

/**
 * One winding of a balanced three winding transformer.
 */
typedef struct {
    size_t bus_id;
    double tap_ratio;
    double phase_shift_degrees;
    double nominal_voltage_kv;
    double rating_a_mva;
    double rating_b_mva;
    double rating_c_mva;
    PioTransformerControlView control;
    bool has_control;
} PioThreeWindingTransformerWindingView;

/**
 * One pairwise impedance of a balanced three winding transformer.
 */
typedef struct {
    double resistance_pu;
    double reactance_pu;
    double base_mva;
} PioThreeWindingTransformerImpedanceView;

/**
 * One balanced control area.
 */
typedef struct {
    size_t number;
    size_t slack_bus_id;
    bool has_slack_bus;
    double net_interchange_mw;
    double tolerance_mw;
    PioStringView name;
    bool has_name;
    PioStringView component_id;
    bool has_component_id;
    PioStringView area_type;
    bool has_area_type;
} PioBalancedAreaView;

/**
 * Coordinate metadata for a multiconductor network.
 */
typedef struct {
    bool has_geo;
    PioStringView space;
    PioStringView crs;
    bool has_crs;
    PioStringView kind;
    bool has_kind;
    bool has_canvas;
    double canvas_width;
    bool has_canvas_width;
    double canvas_height;
    bool has_canvas_height;
    PioStringView canvas_units;
    bool has_canvas_units;
} PioMulticonductorGeoView;

/**
 * Exact table lengths in a multiconductor network.
 *
 * Source extension `extras` maps are not exposed through ABI 7. They are
 * retained by PowerIO for same format emission but are not PowerIO domain data.
 */
typedef struct {
    size_t buses;
    size_t line_codes;
    size_t lines;
    size_t switches;
    size_t transformers;
    size_t loads;
    size_t generators;
    size_t inverter_based_resources;
    size_t control_profiles;
    size_t shunts;
    size_t capacitors;
    size_t voltage_sources;
    size_t untyped_objects;
    size_t commands;
    size_t options;
} PioMulticonductorNetworkCountsView;

/**
 * One point in a multiconductor network coordinate space.
 */
typedef struct {
    double x;
    double y;
    PioStringView kind;
    bool has_kind;
} PioMulticonductorLocationView;

/**
 * One multiconductor bus.
 */
typedef struct {
    PioStringView id;
    size_t terminal_count;
    size_t grounded_terminal_count;
    double voltage_min_v;
    bool has_voltage_min;
    double voltage_max_v;
    bool has_voltage_max;
    /**
     * Nonuniform phase bounds, in phase-terminal order, used when the scalar is absent.
     */
    PioF64View phase_to_ground_voltage_min_v;
    bool has_phase_to_ground_voltage_min;
    PioF64View phase_to_ground_voltage_max_v;
    bool has_phase_to_ground_voltage_max;
    PioF64View phase_to_neutral_voltage_min_v;
    bool has_phase_to_neutral_voltage_min;
    PioF64View phase_to_neutral_voltage_max_v;
    bool has_phase_to_neutral_voltage_max;
    PioF64View phase_to_phase_voltage_min_v;
    bool has_phase_to_phase_voltage_min;
    PioF64View phase_to_phase_voltage_max_v;
    bool has_phase_to_phase_voltage_max;
    double positive_sequence_voltage_min_v;
    bool has_positive_sequence_voltage_min;
    double positive_sequence_voltage_max_v;
    bool has_positive_sequence_voltage_max;
    double negative_sequence_voltage_max_v;
    bool has_negative_sequence_voltage_max;
    double zero_sequence_voltage_max_v;
    bool has_zero_sequence_voltage_max;
    double neutral_to_ground_voltage_max_v;
    bool has_neutral_to_ground_voltage_max;
    PioMulticonductorLocationView location;
    bool has_location;
} PioMulticonductorBusView;

/**
 * One multiconductor line code.
 */
typedef struct {
    PioStringView name;
    size_t conductor_count;
    size_t resistance_matrix_row_count;
    size_t reactance_matrix_row_count;
    size_t conductance_from_matrix_row_count;
    size_t susceptance_from_matrix_row_count;
    size_t conductance_to_matrix_row_count;
    size_t susceptance_to_matrix_row_count;
    PioF64View current_limit_a;
    bool has_current_limit;
    PioF64View apparent_power_limit_va;
    bool has_apparent_power_limit;
    PioStringView source;
    bool has_source;
} PioMulticonductorLineCodeView;

/**
 * One multiconductor line.
 */
typedef struct {
    PioStringView name;
    PioStringView bus_from;
    PioStringView bus_to;
    size_t terminal_map_from_count;
    size_t terminal_map_to_count;
    PioStringView line_code;
    double length_m;
    size_t route_point_count;
    bool has_route;
    PioF64View current_limit_a;
    bool has_current_limit;
    PioF64View apparent_power_limit_va;
    bool has_apparent_power_limit;
} PioMulticonductorLineView;

/**
 * One multiconductor switch.
 */
typedef struct {
    PioStringView name;
    PioStringView bus_from;
    PioStringView bus_to;
    size_t terminal_map_from_count;
    size_t terminal_map_to_count;
    bool open;
    PioF64View current_limit_a;
    bool has_current_limit;
} PioMulticonductorSwitchView;

/**
 * One multiconductor transformer.
 */
typedef struct {
    PioStringView name;
    size_t winding_count;
    PioF64View short_circuit_reactance_percent;
    size_t phase_count;
} PioMulticonductorTransformerView;

/**
 * One winding of a multiconductor transformer.
 */
typedef struct {
    PioStringView bus;
    size_t terminal_map_count;
    PioStringView connection;
    double rated_voltage_v;
    double apparent_power_rating_va;
    double resistance_percent;
    double tap;
    double neutral_resistance_ohm;
    bool has_neutral_resistance;
    double neutral_reactance_ohm;
    bool has_neutral_reactance;
} PioMulticonductorTransformerWindingView;

/**
 * One multiconductor load.
 */
typedef struct {
    PioStringView name;
    PioStringView bus;
    size_t terminal_map_count;
    PioStringView configuration;
    PioF64View active_power_nominal_w;
    PioF64View reactive_power_nominal_var;
    PioStringView voltage_model;
    PioF64View nominal_voltage_v;
    PioF64View active_power_constant_impedance;
    PioF64View active_power_constant_current;
    PioF64View active_power_constant_power;
    PioF64View reactive_power_constant_impedance;
    PioF64View reactive_power_constant_current;
    PioF64View reactive_power_constant_power;
    PioF64View active_power_exponent;
    PioF64View reactive_power_exponent;
} PioMulticonductorLoadView;

/**
 * One multiconductor generator.
 */
typedef struct {
    PioStringView name;
    PioStringView bus;
    size_t terminal_map_count;
    PioStringView configuration;
    PioF64View active_power_nominal_w;
    PioF64View reactive_power_nominal_var;
    PioF64View active_power_min_w;
    bool has_active_power_min;
    PioF64View active_power_max_w;
    bool has_active_power_max;
    PioF64View reactive_power_min_var;
    bool has_reactive_power_min;
    PioF64View reactive_power_max_var;
    bool has_reactive_power_max;
    PioF64View active_power_dispatch_cost_per_kwh;
    bool has_active_power_dispatch_cost;
    PioF64View apparent_power_limit_va;
    bool has_apparent_power_limit;
    PioF64View current_limit_a;
    bool has_current_limit;
} PioMulticonductorGeneratorView;

/**
 * One inverter based resource.
 */
typedef struct {
    PioStringView name;
    PioStringView bus;
    size_t terminal_map_count;
    PioStringView topology;
    PioStringView prime_mover;
    PioF64View apparent_power_limit_va;
    PioF64View current_limit_a;
    bool has_current_limit;
    double active_power_available_w;
    bool has_active_power_available;
    PioF64View active_power_min_w;
    bool has_active_power_min;
    PioF64View active_power_max_w;
    bool has_active_power_max;
    PioF64View reactive_power_min_var;
    bool has_reactive_power_min;
    PioF64View reactive_power_max_var;
    bool has_reactive_power_max;
    PioStringView control_profile;
    bool has_control_profile;
    PioStringView voltage_aggregation;
    bool has_voltage_aggregation;
} PioInverterBasedResourceView;

/**
 * One inverter control profile.
 */
typedef struct {
    PioStringView name;
    bool has_power_factor;
    double power_factor;
    bool has_volt_var;
    PioStringView volt_var_voltage_reference;
    bool has_volt_var_voltage_reference;
    PioF64View volt_var_breakpoints;
    PioF64View volt_var_reactive_power_limits;
    PioStringView volt_var_reactive_power_unit;
    bool has_volt_var_reactive_power_unit;
    PioStringView volt_var_reactive_power_reference;
    bool has_volt_var_reactive_power_reference;
    double volt_var_active_power_min_for_reactive_power_w;
    bool has_volt_var_active_power_min_for_reactive_power;
    double volt_var_active_power_min_for_max_reactive_power_w;
    bool has_volt_var_active_power_min_for_max_reactive_power;
    bool has_volt_watt;
    PioStringView volt_watt_voltage_reference;
    bool has_volt_watt_voltage_reference;
    PioF64View volt_watt_breakpoints;
    PioF64View volt_watt_active_power_limits;
    PioStringView volt_watt_active_power_unit;
    bool has_volt_watt_active_power_unit;
    PioStringView volt_watt_active_power_reference;
    bool has_volt_watt_active_power_reference;
} PioControlProfileView;

/**
 * One multiconductor shunt.
 */
typedef struct {
    PioStringView name;
    PioStringView bus;
    size_t terminal_map_count;
    size_t conductance_matrix_row_count;
    size_t susceptance_matrix_row_count;
} PioMulticonductorShuntView;

/**
 * One multiconductor capacitor.
 */
typedef struct {
    PioStringView name;
    PioStringView bus;
    size_t terminal_map_count;
    PioStringView configuration;
    double rated_reactive_power_var;
    double nominal_voltage_v;
} PioMulticonductorCapacitorView;

/**
 * One multiconductor voltage source.
 */
typedef struct {
    PioStringView name;
    PioStringView bus;
    size_t terminal_map_count;
    PioF64View voltage_magnitude_v;
    PioF64View voltage_angle_rad;
    /**
     * Dollars/kWh in phase order, excluding neutral terminals.
     */
    PioF64View energy_cost_rate_per_kwh;
    bool has_energy_cost_rate;
} PioVoltageSourceView;

/**
 * One source object retained without a typed PowerIO representation.
 */
typedef struct {
    PioStringView class_name;
    PioStringView name;
    size_t property_count;
} PioMulticonductorUntypedObjectView;

/**
 * One property of an untyped source object.
 */
typedef struct {
    PioStringView name;
    bool has_name;
    PioStringView value;
} PioMulticonductorUntypedPropertyView;

/**
 * One retained source command.
 */
typedef struct {
    PioStringView verb;
    PioStringView args;
} PioMulticonductorCommandView;

/**
 * Borrowed binary bytes.
 */
typedef struct {
    const uint8_t *data;
    size_t len;
} PioByteView;

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

/**
 * Return the ABI number compiled into this library.
 */
uint32_t pio_abi_version(void);

/**
 * Return the PowerIO crate version.
 */
PioStringView pio_version(void);

/**
 * The failure's stable diagnostic code.
 */
PioStringView pio_error_code(const PioError *error);

/**
 * The rendered failure message.
 */
PioStringView pio_error_message(const PioError *error);

/**
 * The structured diagnostics that caused the failure.
 */
PioDiagnostics *pio_error_diagnostics(const PioError *error);

PioError *pio_error_retain(const PioError *error);

void pio_error_release(PioError *error);

size_t pio_diagnostics_len(const PioDiagnostics *diagnostics);

PioStringView pio_diagnostic_code(const PioDiagnostics *diagnostics, size_t index);

PioStringView pio_diagnostic_severity(const PioDiagnostics *diagnostics, size_t index);

PioStringView pio_diagnostic_message(const PioDiagnostics *diagnostics, size_t index);

/**
 * Whether this diagnostic has a durable identity.
 */
bool pio_diagnostic_has_id(const PioDiagnostics *diagnostics, size_t index);

/**
 * Borrow this diagnostic's durable identity, or an empty view when absent.
 */
PioStringView pio_diagnostic_id(const PioDiagnostics *diagnostics, size_t index);

/**
 * Whether this diagnostic names a value element.
 */
bool pio_diagnostic_has_target(const PioDiagnostics *diagnostics, size_t index);

/**
 * Borrow this diagnostic's value element locator, or an empty view when absent.
 */
PioStringView pio_diagnostic_target(const PioDiagnostics *diagnostics, size_t index);

/**
 * Whether this diagnostic carries a suggested action.
 */
bool pio_diagnostic_has_suggested_action(const PioDiagnostics *diagnostics, size_t index);

/**
 * Borrow this diagnostic's suggested action, or an empty view when absent.
 */
PioStringView pio_diagnostic_suggested_action(const PioDiagnostics *diagnostics, size_t index);

/**
 * Number of source byte ranges attached to this diagnostic.
 */
size_t pio_diagnostic_n_spans(const PioDiagnostics *diagnostics, size_t index);

/**
 * Read one source byte range. The source string borrows from `diagnostics`.
 */
bool pio_diagnostic_span(const PioDiagnostics *diagnostics,
                         size_t index,
                         size_t span_index,
                         PioDiagnosticSpanView *output,
                         PioError **error);

/**
 * Number of other diagnostic identities referenced by this diagnostic.
 */
size_t pio_diagnostic_n_related(const PioDiagnostics *diagnostics, size_t index);

/**
 * Borrow one related diagnostic identity, or an empty view when out of range.
 */
PioStringView pio_diagnostic_related(const PioDiagnostics *diagnostics,
                                     size_t index,
                                     size_t related_index);

/**
 * Serialize this diagnostic's structured details as an owned JSON object.
 */
PioString *pio_diagnostic_details_json(const PioDiagnostics *diagnostics,
                                       size_t index,
                                       PioError **error);

PioDiagnostics *pio_diagnostics_retain(const PioDiagnostics *diagnostics);

void pio_diagnostics_release(PioDiagnostics *diagnostics);

/**
 * Acquire a file or directory path.
 */
PioSource *pio_source_open(const char *path, size_t path_len, PioError **error);

/**
 * Retain named bytes as an in-memory source. Binary content is supported.
 */
PioSource *pio_source_from_memory(const char *name,
                                  size_t name_len,
                                  const uint8_t *data,
                                  size_t data_len,
                                  PioError **error);

PioSource *pio_source_retain(const PioSource *source);

void pio_source_release(PioSource *source);

/**
 * Select a filesystem output path.
 */
PioDestination *pio_destination_path(const char *path, size_t path_len, PioError **error);

/**
 * Select memory output and prefix returned artifact names with `root`.
 */
PioDestination *pio_destination_memory(const char *root, size_t root_len, PioError **error);

PioDestination *pio_destination_retain(const PioDestination *destination);

void pio_destination_release(PioDestination *destination);

/**
 * Parse one geographic sidecar from an acquired source.
 */
PioGeoLayer *pio_geo_layer_parse(const PioSource *source, PioError **error);

/**
 * Return diagnostics produced while parsing a geographic sidecar.
 */
PioDiagnostics *pio_geo_layer_diagnostics(const PioGeoLayer *layer);

PioGeoLayer *pio_geo_layer_retain(const PioGeoLayer *layer);

void pio_geo_layer_release(PioGeoLayer *layer);

/**
 * Parse one acquired grid exchange source.
 */
PioModule *pio_parse(const PioSource *source,
                     const char *format,
                     size_t format_len,
                     PioError **error);

/**
 * Deserialize one PowerIO IR source.
 *
 * A document carries the independent PowerIO IR generation reported by
 * `pio_schema_report`. This library refuses any unsupported identity or
 * generation through `error`, naming what it found.
 */
PioModule *pio_module_deserialize(const PioSource *source, PioError **error);

/**
 * Construct a DC power flow calculation module from a balanced network module.
 */
PioModule *pio_module_to_dc_pf_instance(const PioModule *module, PioError **error);

/**
 * Construct an AC power flow calculation module from a balanced network module.
 */
PioModule *pio_module_to_ac_pf_instance(const PioModule *module, PioError **error);

/**
 * Construct a DC optimal power flow calculation module from a balanced network module.
 */
PioModule *pio_module_to_dc_opf_instance(const PioModule *module, PioError **error);

/**
 * Construct an AC optimal power flow calculation module from a balanced network module.
 */
PioModule *pio_module_to_ac_opf_instance(const PioModule *module, PioError **error);

/**
 * Construct a multiconductor AC power flow calculation module from a
 * multiconductor network module.
 */
PioModule *pio_module_to_mc_ac_pf_instance(const PioModule *module, PioError **error);

/**
 * Construct a multiconductor AC optimal power flow calculation module from a
 * multiconductor network module.
 */
PioModule *pio_module_to_mc_ac_opf_instance(const PioModule *module, PioError **error);

/**
 * Apply one geographic layer to a balanced or multiconductor network module.
 * The input module is unchanged. When `out_report` is not NULL, it receives
 * an independently owned report handle.
 */
PioModule *pio_module_apply_geo_layer(const PioModule *module,
                                      const PioGeoLayer *layer,
                                      PioGeoApplyReport **out_report,
                                      PioError **error);

size_t pio_geo_apply_report_matched_buses(const PioGeoApplyReport *report);

size_t pio_geo_apply_report_matched_branches(const PioGeoApplyReport *report);

size_t pio_geo_apply_report_unmatched_features(const PioGeoApplyReport *report);

size_t pio_geo_apply_report_unlocated_buses(const PioGeoApplyReport *report);

size_t pio_geo_apply_report_unlocated_branches(const PioGeoApplyReport *report);

size_t pio_geo_apply_report_note_count(const PioGeoApplyReport *report);

PioStringView pio_geo_apply_report_note_at(const PioGeoApplyReport *report,
                                           size_t index,
                                           PioError **error);

PioGeoApplyReport *pio_geo_apply_report_retain(const PioGeoApplyReport *report);

void pio_geo_apply_report_release(PioGeoApplyReport *report);

/**
 * Return an owner-rooted view of the module's value.
 */
PioValueHandle *pio_module_value(const PioModule *module);

/**
 * Return the module's stored diagnostics.
 */
PioDiagnostics *pio_module_diagnostics(const PioModule *module);

/**
 * Read the program identity recorded with a module.
 */
bool pio_module_producer(const PioModule *module, PioModuleProducerView *output, PioError **error);

size_t pio_module_source_count(const PioModule *module);

/**
 * Read one durable source descriptor by zero based position.
 */
bool pio_module_source_at(const PioModule *module,
                          size_t index,
                          PioModuleSourceView *output,
                          PioError **error);

size_t pio_module_source_map_count(const PioModule *module);

/**
 * Read one source map entry by zero based position.
 */
bool pio_module_source_map_at(const PioModule *module,
                              size_t index,
                              PioModuleSourceMapEntryView *output,
                              PioError **error);

/**
 * Read one byte range from a source map entry.
 */
bool pio_module_source_map_span_at(const PioModule *module,
                                   size_t entry_index,
                                   size_t span_index,
                                   PioSourceSpanView *output,
                                   PioError **error);

size_t pio_module_history_count(const PioModule *module);

/**
 * Read one operation from module history by zero based position.
 */
bool pio_module_history_at(const PioModule *module,
                           size_t index,
                           PioModuleHistoryEntryView *output,
                           PioError **error);

/**
 * Read one named structured history parameter by zero based position.
 */
bool pio_module_history_parameter_at(const PioModule *module,
                                     size_t history_index,
                                     size_t parameter_index,
                                     PioModuleHistoryParameterView *output,
                                     PioError **error);

/**
 * Return an owner-rooted structured history parameter value.
 */
PioJsonValue *pio_module_history_parameter_value_at(const PioModule *module,
                                                    size_t history_index,
                                                    size_t parameter_index,
                                                    PioError **error);

/**
 * Read one assumption attached to a history entry.
 */
PioStringView pio_module_history_assumption_at(const PioModule *module,
                                               size_t history_index,
                                               size_t assumption_index,
                                               PioError **error);

/**
 * Read one declared loss attached to a history entry.
 */
PioStringView pio_module_history_loss_at(const PioModule *module,
                                         size_t history_index,
                                         size_t loss_index,
                                         PioError **error);

size_t pio_module_extension_count(const PioModule *module);

/**
 * Read one namespaced structured module extension by zero based position.
 */
bool pio_module_extension_at(const PioModule *module,
                             size_t index,
                             PioModuleExtensionView *output,
                             PioError **error);

/**
 * Return an owner-rooted structured module extension value.
 */
PioJsonValue *pio_module_extension_value_at(const PioModule *module,
                                            size_t index,
                                            PioError **error);

/**
 * Read the type and scalar or collection data for a structured value.
 */
bool pio_json_value_get(const PioJsonValue *value, PioJsonValueView *output, PioError **error);

/**
 * Return one owner-rooted element from a structured JSON array.
 */
PioJsonValue *pio_json_value_array_at(const PioJsonValue *value, size_t index, PioError **error);

/**
 * Read one key and value type from a structured JSON object.
 */
bool pio_json_value_object_entry_at(const PioJsonValue *value,
                                    size_t index,
                                    PioJsonObjectEntryView *output,
                                    PioError **error);

/**
 * Return one owner-rooted value from a structured JSON object by position.
 */
PioJsonValue *pio_json_value_object_value_at(const PioJsonValue *value,
                                             size_t index,
                                             PioError **error);

PioJsonValue *pio_json_value_retain(const PioJsonValue *value);

void pio_json_value_release(PioJsonValue *value);

PioModule *pio_module_retain(const PioModule *module);

void pio_module_release(PioModule *module);

/**
 * Canonical structural type name, such as `powerio.BalancedNetwork`.
 */
PioStringView pio_value_type_name(const PioValueHandle *value);

/**
 * Exact structural type predicate.
 */
bool pio_value_is_type(const PioValueHandle *value, const char *type_name, size_t type_name_len);

/**
 * Borrow the value as a balanced network without serialization or copying.
 */
PioBalancedNetwork *pio_value_balanced_network(const PioValueHandle *value, PioError **error);

/**
 * Take the value as a geographic layer handle. The layer is copied out of
 * the value, so the handle outlives the module the way
 * `pio_geo_layer_parse` produces one.
 */
PioGeoLayer *pio_value_geo_layer(const PioValueHandle *value, PioError **error);

/**
 * Borrow the value as a multiconductor network without serialization or copying.
 */
PioMulticonductorNetwork *pio_value_multiconductor_network(const PioValueHandle *value,
                                                           PioError **error);

/**
 * Borrow the value as a time series.
 */
PioTimeSeriesHandle *pio_value_time_series(const PioValueHandle *value, PioError **error);

/**
 * Borrow the value as a scenario set.
 */
PioScenarioSetHandle *pio_value_scenario_set(const PioValueHandle *value, PioError **error);

PioOperatingPoint *pio_value_balanced_operating_point(const PioValueHandle *value,
                                                      PioError **error);

PioOperatingPoint *pio_value_multiconductor_operating_point(const PioValueHandle *value,
                                                            PioError **error);

PioCalculationInstance *pio_value_dc_pf_instance(const PioValueHandle *value, PioError **error);

PioCalculationInstance *pio_value_ac_pf_instance(const PioValueHandle *value, PioError **error);

PioCalculationInstance *pio_value_dc_opf_instance(const PioValueHandle *value, PioError **error);

PioCalculationInstance *pio_value_ac_opf_instance(const PioValueHandle *value, PioError **error);

PioCalculationInstance *pio_value_mc_ac_pf_instance(const PioValueHandle *value, PioError **error);

PioCalculationInstance *pio_value_mc_ac_opf_instance(const PioValueHandle *value, PioError **error);

PioCalculationInstance *pio_value_ac_scuc_instance(const PioValueHandle *value, PioError **error);

PioCalculationSolution *pio_value_dc_pf_solution(const PioValueHandle *value, PioError **error);

PioCalculationSolution *pio_value_ac_pf_solution(const PioValueHandle *value, PioError **error);

PioCalculationSolution *pio_value_dc_opf_solution(const PioValueHandle *value, PioError **error);

PioCalculationSolution *pio_value_ac_opf_solution(const PioValueHandle *value, PioError **error);

PioCalculationSolution *pio_value_socwr_opf_solution(const PioValueHandle *value, PioError **error);

PioCalculationSolution *pio_value_mc_ac_pf_solution(const PioValueHandle *value, PioError **error);

PioCalculationSolution *pio_value_mc_ac_opf_solution(const PioValueHandle *value, PioError **error);

PioCalculationSolution *pio_value_ac_scuc_solution(const PioValueHandle *value, PioError **error);

PioStringView pio_operating_point_type_name(const PioOperatingPoint *point);

PioStringView pio_calculation_instance_type_name(const PioCalculationInstance *instance);

PioStringView pio_calculation_solution_type_name(const PioCalculationSolution *solution);

/**
 * Return an owner-rooted view of the exact calculation instance retained by
 * a calculation solution.
 */
PioCalculationInstance *pio_calculation_solution_instance(const PioCalculationSolution *solution,
                                                          PioError **error);

PioBalancedNetwork *pio_operating_point_balanced_network(const PioOperatingPoint *point,
                                                         PioError **error);

PioMulticonductorNetwork *pio_operating_point_multiconductor_network(const PioOperatingPoint *point,
                                                                     PioError **error);

PioBalancedNetwork *pio_calculation_instance_balanced_network(const PioCalculationInstance *instance,
                                                              PioError **error);

PioMulticonductorNetwork *pio_calculation_instance_multiconductor_network(const PioCalculationInstance *instance,
                                                                          PioError **error);

/**
 * Build the matrix free DC OPF inputs from one typed instance.
 */
PioDcOpfPreparation *pio_build_dc_opf_preparation(const PioCalculationInstance *instance,
                                                  const char *units,
                                                  size_t units_len,
                                                  bool skip_zero_impedance,
                                                  bool synthesize_unrated_limits,
                                                  bool correct_angle_difference_bounds,
                                                  PioError **error);

/**
 * Read the dimensions and conventions of a DC OPF preparation.
 */
bool pio_dc_opf_preparation_summary(const PioDcOpfPreparation *preparation,
                                    PioDcOpfPreparationView *output,
                                    PioError **error);

/**
 * Borrow the dense reference bus indices of a DC OPF preparation.
 */
PioSizeView pio_dc_opf_preparation_reference_buses(const PioDcOpfPreparation *preparation);

/**
 * Borrow the analysis rows skipped for zero impedance.
 */
PioSizeView pio_dc_opf_preparation_skipped_zero_impedance(const PioDcOpfPreparation *preparation);

/**
 * Read one dense bus row of a DC OPF preparation.
 */
bool pio_dc_opf_preparation_bus_at(const PioDcOpfPreparation *preparation,
                                   size_t index,
                                   PioDcOpfBusView *output,
                                   PioError **error);

/**
 * Read one generator row of a DC OPF preparation.
 */
bool pio_dc_opf_preparation_generator_at(const PioDcOpfPreparation *preparation,
                                         size_t index,
                                         PioDcOpfGeneratorView *output,
                                         PioError **error);

/**
 * Read one active branch row of a DC OPF preparation.
 */
bool pio_dc_opf_preparation_branch_at(const PioDcOpfPreparation *preparation,
                                      size_t index,
                                      PioDcOpfBranchView *output,
                                      PioError **error);

/**
 * Build the matrix free AC OPF inputs from one typed instance.
 */
PioAcOpfPreparation *pio_build_ac_opf_preparation(const PioCalculationInstance *instance,
                                                  const char *units,
                                                  size_t units_len,
                                                  bool skip_zero_impedance,
                                                  bool synthesize_unrated_limits,
                                                  bool correct_angle_difference_bounds,
                                                  PioError **error);

/**
 * Read the dimensions and conventions of an AC OPF preparation.
 */
bool pio_ac_opf_preparation_summary(const PioAcOpfPreparation *preparation,
                                    PioAcOpfPreparationView *output,
                                    PioError **error);

/**
 * Borrow the dense reference bus indices of an AC OPF preparation.
 */
PioSizeView pio_ac_opf_preparation_reference_buses(const PioAcOpfPreparation *preparation);

/**
 * Borrow the analysis rows skipped for zero impedance.
 */
PioSizeView pio_ac_opf_preparation_skipped_zero_impedance(const PioAcOpfPreparation *preparation);

/**
 * Read one dense bus row of an AC OPF preparation.
 */
bool pio_ac_opf_preparation_bus_at(const PioAcOpfPreparation *preparation,
                                   size_t index,
                                   PioAcOpfBusView *output,
                                   PioError **error);

/**
 * Read one generator row of an AC OPF preparation.
 */
bool pio_ac_opf_preparation_generator_at(const PioAcOpfPreparation *preparation,
                                         size_t index,
                                         PioAcOpfGeneratorView *output,
                                         PioError **error);

/**
 * Read one storage row of an AC OPF preparation.
 */
bool pio_ac_opf_preparation_storage_at(const PioAcOpfPreparation *preparation,
                                       size_t index,
                                       PioAcOpfStorageView *output,
                                       PioError **error);

/**
 * Read one active branch row of an AC OPF preparation.
 */
bool pio_ac_opf_preparation_branch_at(const PioAcOpfPreparation *preparation,
                                      size_t index,
                                      PioAcOpfBranchView *output,
                                      PioError **error);

size_t pio_dc_pf_instance_bus_specification_count(const PioCalculationInstance *instance);

/**
 * Read one DC power flow bus specification by zero based bus table position.
 */
bool pio_dc_pf_instance_bus_specification_at(const PioCalculationInstance *instance,
                                             size_t index,
                                             PioDcBusSpecificationView *output,
                                             PioError **error);

PioStringView pio_dc_pf_instance_branch_susceptance_formula(const PioCalculationInstance *instance);

size_t pio_ac_pf_instance_bus_specification_count(const PioCalculationInstance *instance);

/**
 * Read one AC power flow bus specification by zero based bus table position.
 */
bool pio_ac_pf_instance_bus_specification_at(const PioCalculationInstance *instance,
                                             size_t index,
                                             PioAcBusSpecificationView *output,
                                             PioError **error);

PioStringView pio_dc_opf_instance_branch_susceptance_formula(const PioCalculationInstance *instance);

size_t pio_calculation_instance_objective_term_count(const PioCalculationInstance *instance);

/**
 * Read one typed objective term by zero based position.
 */
bool pio_calculation_instance_objective_term_at(const PioCalculationInstance *instance,
                                                size_t index,
                                                PioObjectiveTermView *output,
                                                PioError **error);

size_t pio_calculation_instance_active_constraint_count(const PioCalculationInstance *instance);

/**
 * Read one active constraint family by zero based position.
 */
bool pio_calculation_instance_active_constraint_at(const PioCalculationInstance *instance,
                                                   size_t index,
                                                   PioActiveConstraintView *output,
                                                   PioError **error);

/**
 * Read one selected component identity from an `only` constraint selection.
 */
PioStringView pio_calculation_instance_active_constraint_identity_at(const PioCalculationInstance *instance,
                                                                     size_t constraint_index,
                                                                     size_t identity_index,
                                                                     PioError **error);

bool pio_calculation_instance_has_initial_point(const PioCalculationInstance *instance);

/**
 * Return the optional owner-rooted initial operating point. A calculation
 * instance with no initial point returns NULL without setting an error.
 */
PioOperatingPoint *pio_calculation_instance_initial_point(const PioCalculationInstance *instance,
                                                          PioError **error);

size_t pio_mc_ac_pf_instance_load_count(const PioCalculationInstance *instance);

bool pio_mc_ac_pf_instance_load_at(const PioCalculationInstance *instance,
                                   size_t index,
                                   PioPrescribedTerminalPowerView *output,
                                   PioError **error);

bool pio_mc_ac_pf_instance_load_terminal_at(const PioCalculationInstance *instance,
                                            size_t load_index,
                                            size_t terminal_index,
                                            PioTerminalPowerView *output,
                                            PioError **error);

size_t pio_mc_ac_pf_instance_source_count(const PioCalculationInstance *instance);

bool pio_mc_ac_pf_instance_source_at(const PioCalculationInstance *instance,
                                     size_t index,
                                     PioPrescribedSourceVoltageView *output,
                                     PioError **error);

bool pio_mc_ac_pf_instance_source_terminal_at(const PioCalculationInstance *instance,
                                              size_t source_index,
                                              size_t terminal_index,
                                              PioTerminalVoltageView *output,
                                              PioError **error);

size_t pio_mc_ac_pf_instance_isolated_terminal_count(const PioCalculationInstance *instance);

bool pio_mc_ac_pf_instance_isolated_terminal_at(const PioCalculationInstance *instance,
                                                size_t index,
                                                PioIsolatedTerminalView *output,
                                                PioError **error);

size_t pio_mc_ac_pf_instance_active_control_count(const PioCalculationInstance *instance);

bool pio_mc_ac_pf_instance_active_control_at(const PioCalculationInstance *instance,
                                             size_t index,
                                             PioActiveControlView *output,
                                             PioError **error);

/**
 * Read semantic collection sizes for one AC SCUC instance.
 */
bool pio_ac_scuc_instance_dimensions(const PioCalculationInstance *instance,
                                     PioScucDimensionsView *output,
                                     PioError **error);

/**
 * Borrow interval durations in hours, in chronological order.
 */
PioF64View pio_ac_scuc_instance_interval_durations(const PioCalculationInstance *instance);

/**
 * Read the four required violation costs.
 */
bool pio_ac_scuc_instance_violation_costs(const PioCalculationInstance *instance,
                                          PioScucViolationCostView *output,
                                          PioError **error);

size_t pio_ac_scuc_instance_device_count(const PioCalculationInstance *instance);

bool pio_ac_scuc_instance_device_at(const PioCalculationInstance *instance,
                                    size_t index,
                                    PioScucDeviceView *output,
                                    PioError **error);

/**
 * Read one device by its exact source UID.
 */
bool pio_ac_scuc_instance_device_get(const PioCalculationInstance *instance,
                                     const char *uid,
                                     size_t uid_len,
                                     PioScucDeviceView *output,
                                     PioError **error);

size_t pio_ac_scuc_instance_device_startup_cost_adjustment_count(const PioCalculationInstance *instance,
                                                                 size_t device_index);

bool pio_ac_scuc_instance_device_startup_cost_adjustment_at(const PioCalculationInstance *instance,
                                                            size_t device_index,
                                                            size_t adjustment_index,
                                                            PioScucStartupCostAdjustmentView *output,
                                                            PioError **error);

size_t pio_ac_scuc_instance_device_startup_limit_count(const PioCalculationInstance *instance,
                                                       size_t device_index);

bool pio_ac_scuc_instance_device_startup_limit_at(const PioCalculationInstance *instance,
                                                  size_t device_index,
                                                  size_t limit_index,
                                                  PioScucStartupLimitView *output,
                                                  PioError **error);

size_t pio_ac_scuc_instance_device_energy_upper_bound_count(const PioCalculationInstance *instance,
                                                            size_t device_index);

bool pio_ac_scuc_instance_device_energy_upper_bound_at(const PioCalculationInstance *instance,
                                                       size_t device_index,
                                                       size_t requirement_index,
                                                       PioScucEnergyRequirementView *output,
                                                       PioError **error);

size_t pio_ac_scuc_instance_device_energy_lower_bound_count(const PioCalculationInstance *instance,
                                                            size_t device_index);

bool pio_ac_scuc_instance_device_energy_lower_bound_at(const PioCalculationInstance *instance,
                                                       size_t device_index,
                                                       size_t requirement_index,
                                                       PioScucEnergyRequirementView *output,
                                                       PioError **error);

bool pio_ac_scuc_instance_device_period_at(const PioCalculationInstance *instance,
                                           size_t device_index,
                                           size_t period_index,
                                           PioScucDevicePeriodView *output,
                                           PioError **error);

size_t pio_ac_scuc_instance_device_energy_cost_block_count(const PioCalculationInstance *instance,
                                                           size_t device_index,
                                                           size_t period_index);

bool pio_ac_scuc_instance_device_energy_cost_block_at(const PioCalculationInstance *instance,
                                                      size_t device_index,
                                                      size_t period_index,
                                                      size_t block_index,
                                                      PioScucEnergyCostBlockView *output,
                                                      PioError **error);

size_t pio_ac_scuc_instance_shunt_count(const PioCalculationInstance *instance);

bool pio_ac_scuc_instance_shunt_at(const PioCalculationInstance *instance,
                                   size_t index,
                                   PioScucShuntView *output,
                                   PioError **error);

/**
 * Read one shunt by its exact source UID.
 */
bool pio_ac_scuc_instance_shunt_get(const PioCalculationInstance *instance,
                                    const char *uid,
                                    size_t uid_len,
                                    PioScucShuntView *output,
                                    PioError **error);

size_t pio_ac_scuc_instance_branch_switching_cost_count(const PioCalculationInstance *instance);

bool pio_ac_scuc_instance_branch_switching_cost_at(const PioCalculationInstance *instance,
                                                   size_t index,
                                                   PioScucBranchSwitchingCostView *output,
                                                   PioError **error);

size_t pio_ac_scuc_instance_transformer_control_count(const PioCalculationInstance *instance);

bool pio_ac_scuc_instance_transformer_control_at(const PioCalculationInstance *instance,
                                                 size_t index,
                                                 PioScucTransformerControlView *output,
                                                 PioError **error);

size_t pio_ac_scuc_instance_active_reserve_zone_count(const PioCalculationInstance *instance);

bool pio_ac_scuc_instance_active_reserve_zone_at(const PioCalculationInstance *instance,
                                                 size_t index,
                                                 PioScucActiveReserveZoneView *output,
                                                 PioError **error);

bool pio_ac_scuc_instance_active_reserve_zone_period_at(const PioCalculationInstance *instance,
                                                        size_t zone_index,
                                                        size_t period_index,
                                                        PioScucActiveReservePeriodView *output,
                                                        PioError **error);

bool pio_ac_scuc_instance_active_reserve_zone_bus_at(const PioCalculationInstance *instance,
                                                     size_t zone_index,
                                                     size_t bus_index,
                                                     PioComponentIdView *output,
                                                     PioError **error);

size_t pio_ac_scuc_instance_reactive_reserve_zone_count(const PioCalculationInstance *instance);

bool pio_ac_scuc_instance_reactive_reserve_zone_at(const PioCalculationInstance *instance,
                                                   size_t index,
                                                   PioScucReactiveReserveZoneView *output,
                                                   PioError **error);

bool pio_ac_scuc_instance_reactive_reserve_zone_period_at(const PioCalculationInstance *instance,
                                                          size_t zone_index,
                                                          size_t period_index,
                                                          PioScucReactiveReservePeriodView *output,
                                                          PioError **error);

bool pio_ac_scuc_instance_reactive_reserve_zone_bus_at(const PioCalculationInstance *instance,
                                                       size_t zone_index,
                                                       size_t bus_index,
                                                       PioComponentIdView *output,
                                                       PioError **error);

/**
 * Return the number of named contingencies.
 */
size_t pio_ac_scuc_instance_contingency_count(const PioCalculationInstance *instance);

/**
 * Read one named contingency in source order.
 */
bool pio_ac_scuc_instance_contingency_at(const PioCalculationInstance *instance,
                                         size_t contingency_index,
                                         PioScucContingencyView *output,
                                         PioError **error);

/**
 * Read one named contingency by its exact source UID.
 */
bool pio_ac_scuc_instance_contingency_get(const PioCalculationInstance *instance,
                                          const char *uid,
                                          size_t uid_len,
                                          PioScucContingencyView *output,
                                          PioError **error);

/**
 * Read one stable component identity from a named contingency.
 */
bool pio_ac_scuc_instance_contingency_component_at(const PioCalculationInstance *instance,
                                                   size_t contingency_index,
                                                   size_t component_index,
                                                   PioScucContingencyComponentView *output,
                                                   PioError **error);

PioBalancedNetwork *pio_calculation_solution_balanced_network(const PioCalculationSolution *solution,
                                                              PioError **error);

PioMulticonductorNetwork *pio_calculation_solution_multiconductor_network(const PioCalculationSolution *solution,
                                                                          PioError **error);

/**
 * Read one operating point quantity by its PowerIO quantity name and stable
 * component identity. Multiconductor terminal identities use
 * component/terminal. Returns false when the point does not contain the
 * quantity or identity.
 */
bool pio_operating_point_get_value(const PioOperatingPoint *point,
                                   const char *quantity,
                                   size_t quantity_len,
                                   const char *identity,
                                   size_t identity_len,
                                   double *out_value,
                                   PioError **error);

PioOperatingPoint *pio_operating_point_retain(const PioOperatingPoint *point);

void pio_operating_point_release(PioOperatingPoint *point);

PioCalculationInstance *pio_calculation_instance_retain(const PioCalculationInstance *instance);

void pio_calculation_instance_release(PioCalculationInstance *instance);

PioDcOpfPreparation *pio_dc_opf_preparation_retain(const PioDcOpfPreparation *preparation);

void pio_dc_opf_preparation_release(PioDcOpfPreparation *preparation);

PioAcOpfPreparation *pio_ac_opf_preparation_retain(const PioAcOpfPreparation *preparation);

void pio_ac_opf_preparation_release(PioAcOpfPreparation *preparation);

PioCalculationSolution *pio_calculation_solution_retain(const PioCalculationSolution *solution);

void pio_calculation_solution_release(PioCalculationSolution *solution);

PioValueHandle *pio_value_retain(const PioValueHandle *value);

void pio_value_release(PioValueHandle *value);

size_t pio_time_series_len(const PioTimeSeriesHandle *series);

PioStringView pio_time_series_element_type(const PioTimeSeriesHandle *series);

/**
 * Return an owner-rooted entry by zero-based position.
 */
PioValueHandle *pio_time_series_get(const PioTimeSeriesHandle *series,
                                    size_t index,
                                    PioError **error);

PioTimeSeriesHandle *pio_time_series_retain(const PioTimeSeriesHandle *series);

void pio_time_series_release(PioTimeSeriesHandle *series);

size_t pio_scenario_set_len(const PioScenarioSetHandle *set);

PioStringView pio_scenario_set_element_type(const PioScenarioSetHandle *set);

PioStringView pio_scenario_set_id_at(const PioScenarioSetHandle *set, size_t index);

/**
 * Return an owner-rooted scenario value by zero-based position.
 */
PioValueHandle *pio_scenario_set_get_at(const PioScenarioSetHandle *set,
                                        size_t index,
                                        PioError **error);

/**
 * Return an owner-rooted scenario value by exact scenario ID.
 */
PioValueHandle *pio_scenario_set_get(const PioScenarioSetHandle *set,
                                     const char *id,
                                     size_t id_len,
                                     PioError **error);

PioScenarioSetHandle *pio_scenario_set_retain(const PioScenarioSetHandle *set);

void pio_scenario_set_release(PioScenarioSetHandle *set);

PioStringView pio_balanced_network_name(const PioBalancedNetwork *network);

double pio_balanced_network_base_mva(const PioBalancedNetwork *network);

double pio_balanced_network_base_frequency_hz(const PioBalancedNetwork *network);

/**
 * Read the optional coordinate space metadata for a balanced network.
 */
bool pio_balanced_network_geo(const PioBalancedNetwork *network,
                              PioBalancedGeoView *output,
                              PioError **error);

bool pio_balanced_network_has_detailed_connectivity(const PioBalancedNetwork *network);

/**
 * Return the optional owner-rooted detailed connectivity view.
 */
PioDetailedConnectivity *pio_balanced_network_detailed_connectivity(const PioBalancedNetwork *network);

/**
 * Read every detailed connectivity table length.
 */
bool pio_detailed_connectivity_counts(const PioDetailedConnectivity *details,
                                      PioDetailedConnectivityCountsView *output,
                                      PioError **error);

/**
 * Read one field that was absent from the source representation.
 */
bool pio_detailed_connectivity_omitted_field_at(const PioDetailedConnectivity *details,
                                                size_t index,
                                                PioOmittedFieldView *output,
                                                PioError **error);

/**
 * Read one component metadata record by zero based table position.
 */
bool pio_detailed_connectivity_component_metadata_at(const PioDetailedConnectivity *details,
                                                     size_t index,
                                                     PioComponentMetadataView *output,
                                                     PioError **error);

/**
 * Read one alias from a component metadata record.
 */
bool pio_detailed_connectivity_component_alias_at(const PioDetailedConnectivity *details,
                                                  size_t metadata_index,
                                                  size_t alias_index,
                                                  PioComponentAliasView *output,
                                                  PioError **error);

/**
 * Read one external identifier from a component metadata record.
 */
bool pio_detailed_connectivity_external_identifier_at(const PioDetailedConnectivity *details,
                                                      size_t metadata_index,
                                                      size_t identifier_index,
                                                      PioExternalIdentifierView *output,
                                                      PioError **error);

/**
 * Read one string property from a component metadata record.
 */
bool pio_detailed_connectivity_component_property_at(const PioDetailedConnectivity *details,
                                                     size_t metadata_index,
                                                     size_t property_index,
                                                     PioStringPropertyView *output,
                                                     PioError **error);

/**
 * Read one PowSybl subnetwork by zero based table position.
 */
bool pio_detailed_connectivity_subnetwork_at(const PioDetailedConnectivity *details,
                                             size_t index,
                                             PioSubnetworkView *output,
                                             PioError **error);

/**
 * Read one component identity contained by a PowSybl subnetwork.
 */
bool pio_detailed_connectivity_subnetwork_component_at(const PioDetailedConnectivity *details,
                                                       size_t subnetwork_index,
                                                       size_t component_index,
                                                       PioComponentIdView *output,
                                                       PioError **error);

/**
 * Read one substation by zero based table position.
 */
bool pio_detailed_connectivity_substation_at(const PioDetailedConnectivity *details,
                                             size_t index,
                                             PioSubstationView *output,
                                             PioError **error);

/**
 * Read one geographical tag of a substation.
 */
bool pio_detailed_connectivity_substation_geographical_tag_at(const PioDetailedConnectivity *details,
                                                              size_t substation_index,
                                                              size_t tag_index,
                                                              PioStringView *output,
                                                              PioError **error);

/**
 * Read one voltage level by zero based table position.
 */
bool pio_detailed_connectivity_voltage_level_at(const PioDetailedConnectivity *details,
                                                size_t index,
                                                PioVoltageLevelView *output,
                                                PioError **error);

/**
 * Read one balanced bus ID assigned to a voltage level.
 */
bool pio_detailed_connectivity_voltage_level_bus_at(const PioDetailedConnectivity *details,
                                                    size_t voltage_level_index,
                                                    size_t bus_index,
                                                    size_t *output,
                                                    PioError **error);

/**
 * Read one configured bus breaker bus by zero based table position.
 */
bool pio_detailed_connectivity_bus_breaker_bus_at(const PioDetailedConnectivity *details,
                                                  size_t index,
                                                  PioBusBreakerBusView *output,
                                                  PioError **error);

/**
 * Read one calculated bus by zero based table position.
 */
bool pio_detailed_connectivity_calculated_bus_at(const PioDetailedConnectivity *details,
                                                 size_t index,
                                                 PioCalculatedBusView *output,
                                                 PioError **error);

/**
 * Read one node identity from a calculated bus.
 */
bool pio_detailed_connectivity_calculated_bus_node_at(const PioDetailedConnectivity *details,
                                                      size_t calculated_bus_index,
                                                      size_t node_index,
                                                      PioComponentIdView *output,
                                                      PioError **error);

/**
 * Read one connectivity node by zero based table position.
 */
bool pio_detailed_connectivity_node_at(const PioDetailedConnectivity *details,
                                       size_t index,
                                       PioConnectivityNodeView *output,
                                       PioError **error);

/**
 * Read one busbar section by zero based table position.
 */
bool pio_detailed_connectivity_busbar_section_at(const PioDetailedConnectivity *details,
                                                 size_t index,
                                                 PioBusbarSectionView *output,
                                                 PioError **error);

/**
 * Read one CIM junction by zero based table position.
 */
bool pio_detailed_connectivity_junction_at(const PioDetailedConnectivity *details,
                                           size_t index,
                                           PioJunctionView *output,
                                           PioError **error);

/**
 * Read one AC terminal by zero based table position.
 */
bool pio_detailed_connectivity_terminal_at(const PioDetailedConnectivity *details,
                                           size_t index,
                                           PioDetailedTerminalView *output,
                                           PioError **error);

/**
 * Read one detailed topology switch by zero based table position.
 */
bool pio_detailed_connectivity_switch_at(const PioDetailedConnectivity *details,
                                         size_t index,
                                         PioTopologySwitchView *output,
                                         PioError **error);

/**
 * Read one node breaker internal connection by zero based table position.
 */
bool pio_detailed_connectivity_internal_connection_at(const PioDetailedConnectivity *details,
                                                      size_t index,
                                                      PioInternalConnectionView *output,
                                                      PioError **error);

/**
 * Read one operational limit group by zero based table position.
 */
bool pio_detailed_connectivity_operational_limit_group_at(const PioDetailedConnectivity *details,
                                                          size_t index,
                                                          PioOperationalLimitGroupView *output,
                                                          PioError **error);

/**
 * Read one string property from an operational limit group.
 */
bool pio_detailed_connectivity_operational_limit_group_property_at(const PioDetailedConnectivity *details,
                                                                   size_t group_index,
                                                                   size_t property_index,
                                                                   PioStringPropertyView *output,
                                                                   PioError **error);

/**
 * Read one temporary current, active power, or apparent power limit.
 */
bool pio_detailed_connectivity_temporary_limit_at(const PioDetailedConnectivity *details,
                                                  size_t group_index,
                                                  const char *quantity,
                                                  size_t quantity_len,
                                                  size_t limit_index,
                                                  PioTemporaryLimitView *output,
                                                  PioError **error);

/**
 * Read one PowSybl boundary line by zero based table position.
 */
bool pio_detailed_connectivity_boundary_line_at(const PioDetailedConnectivity *details,
                                                size_t index,
                                                PioBoundaryLineView *output,
                                                PioError **error);

/**
 * Read one property on a boundary line generation reactive limit record.
 */
bool pio_detailed_connectivity_boundary_line_reactive_limit_property_at(const PioDetailedConnectivity *details,
                                                                        size_t boundary_line_index,
                                                                        size_t property_index,
                                                                        PioStringPropertyView *output,
                                                                        PioError **error);

/**
 * Read one point from a boundary line generation reactive capability curve.
 */
bool pio_detailed_connectivity_boundary_line_reactive_capability_point_at(const PioDetailedConnectivity *details,
                                                                          size_t boundary_line_index,
                                                                          size_t point_index,
                                                                          PioReactiveCapabilityCurvePointView *output,
                                                                          PioError **error);

/**
 * Read one property from one boundary line reactive capability curve point.
 */
bool pio_detailed_connectivity_boundary_line_reactive_capability_point_property_at(const PioDetailedConnectivity *details,
                                                                                   size_t boundary_line_index,
                                                                                   size_t point_index,
                                                                                   size_t property_index,
                                                                                   PioStringPropertyView *output,
                                                                                   PioError **error);

/**
 * Read one PowSybl tie line by zero based table position.
 */
bool pio_detailed_connectivity_tie_line_at(const PioDetailedConnectivity *details,
                                           size_t index,
                                           PioTieLineView *output,
                                           PioError **error);

/**
 * Read one transformer tap changer by zero based table position.
 */
bool pio_detailed_connectivity_tap_changer_at(const PioDetailedConnectivity *details,
                                              size_t index,
                                              PioTapChangerView *output,
                                              PioError **error);

/**
 * Read one transformer tap changer step by zero based position.
 */
bool pio_detailed_connectivity_tap_changer_step_at(const PioDetailedConnectivity *details,
                                                   size_t tap_changer_index,
                                                   size_t step_index,
                                                   PioTapChangerStepView *output,
                                                   PioError **error);

/**
 * Read reactive limits retained for one equipment record.
 */
bool pio_detailed_connectivity_equipment_reactive_limits_at(const PioDetailedConnectivity *details,
                                                            size_t index,
                                                            PioEquipmentReactiveLimitsView *output,
                                                            PioError **error);

/**
 * Read one property from an equipment reactive limit record.
 */
bool pio_detailed_connectivity_equipment_reactive_limit_property_at(const PioDetailedConnectivity *details,
                                                                    size_t equipment_index,
                                                                    size_t property_index,
                                                                    PioStringPropertyView *output,
                                                                    PioError **error);

/**
 * Read one point from an equipment reactive capability curve.
 */
bool pio_detailed_connectivity_equipment_reactive_capability_point_at(const PioDetailedConnectivity *details,
                                                                      size_t equipment_index,
                                                                      size_t point_index,
                                                                      PioReactiveCapabilityCurvePointView *output,
                                                                      PioError **error);

/**
 * Read one property from an equipment reactive capability curve point.
 */
bool pio_detailed_connectivity_equipment_reactive_capability_point_property_at(const PioDetailedConnectivity *details,
                                                                               size_t equipment_index,
                                                                               size_t point_index,
                                                                               size_t property_index,
                                                                               PioStringPropertyView *output,
                                                                               PioError **error);

/**
 * Read one DC converter unit by zero based table position.
 */
bool pio_detailed_connectivity_dc_converter_unit_at(const PioDetailedConnectivity *details,
                                                    size_t index,
                                                    PioDcConverterUnitView *output,
                                                    PioError **error);

/**
 * Read one DC topological node by zero based table position.
 */
bool pio_detailed_connectivity_dc_topological_node_at(const PioDetailedConnectivity *details,
                                                      size_t index,
                                                      PioDcNodeView *output,
                                                      PioError **error);

/**
 * Read one physical DC node by zero based table position.
 */
bool pio_detailed_connectivity_dc_node_at(const PioDetailedConnectivity *details,
                                          size_t index,
                                          PioDcNodeView *output,
                                          PioError **error);

/**
 * Read one DC ground by zero based table position.
 */
bool pio_detailed_connectivity_dc_ground_at(const PioDetailedConnectivity *details,
                                            size_t index,
                                            PioDcEquipmentView *output,
                                            PioError **error);

/**
 * Read one DC busbar by zero based table position.
 */
bool pio_detailed_connectivity_dc_busbar_at(const PioDetailedConnectivity *details,
                                            size_t index,
                                            PioDcEquipmentView *output,
                                            PioError **error);

/**
 * Read one DC line by zero based table position.
 */
bool pio_detailed_connectivity_dc_line_at(const PioDetailedConnectivity *details,
                                          size_t index,
                                          PioDcEquipmentView *output,
                                          PioError **error);

/**
 * Read one DC series device by zero based table position.
 */
bool pio_detailed_connectivity_dc_series_device_at(const PioDetailedConnectivity *details,
                                                   size_t index,
                                                   PioDcEquipmentView *output,
                                                   PioError **error);

/**
 * Read one DC switch by zero based table position.
 */
bool pio_detailed_connectivity_dc_switch_at(const PioDetailedConnectivity *details,
                                            size_t index,
                                            PioDcEquipmentView *output,
                                            PioError **error);

/**
 * Read one voltage source converter by zero based table position.
 */
bool pio_detailed_connectivity_voltage_source_converter_at(const PioDetailedConnectivity *details,
                                                           size_t index,
                                                           PioAcDcConverterView *output,
                                                           PioError **error);

/**
 * Read one line commutated converter by zero based table position.
 */
bool pio_detailed_connectivity_line_commutated_converter_at(const PioDetailedConnectivity *details,
                                                            size_t index,
                                                            PioAcDcConverterView *output,
                                                            PioError **error);

/**
 * Read one DC voltage droop curve segment from a voltage source converter.
 */
bool pio_detailed_connectivity_voltage_source_converter_droop_curve_segment_at(const PioDetailedConnectivity *details,
                                                                               size_t converter_index,
                                                                               size_t segment_index,
                                                                               PioDroopCurveSegmentView *output,
                                                                               PioError **error);

/**
 * Read one DC voltage droop curve segment from a line commutated converter.
 */
bool pio_detailed_connectivity_line_commutated_converter_droop_curve_segment_at(const PioDetailedConnectivity *details,
                                                                                size_t converter_index,
                                                                                size_t segment_index,
                                                                                PioDroopCurveSegmentView *output,
                                                                                PioError **error);

/**
 * Read one property from a voltage source converter reactive limit record.
 */
bool pio_detailed_connectivity_voltage_source_converter_reactive_limit_property_at(const PioDetailedConnectivity *details,
                                                                                   size_t converter_index,
                                                                                   size_t property_index,
                                                                                   PioStringPropertyView *output,
                                                                                   PioError **error);

/**
 * Read one point from a voltage source converter reactive capability curve.
 */
bool pio_detailed_connectivity_voltage_source_converter_reactive_capability_point_at(const PioDetailedConnectivity *details,
                                                                                     size_t converter_index,
                                                                                     size_t point_index,
                                                                                     PioReactiveCapabilityCurvePointView *output,
                                                                                     PioError **error);

/**
 * Read one property from a voltage source converter reactive capability point.
 */
bool pio_detailed_connectivity_voltage_source_converter_reactive_capability_point_property_at(const PioDetailedConnectivity *details,
                                                                                              size_t converter_index,
                                                                                              size_t point_index,
                                                                                              size_t property_index,
                                                                                              PioStringPropertyView *output,
                                                                                              PioError **error);

PioDetailedConnectivity *pio_detailed_connectivity_retain(const PioDetailedConnectivity *details);

void pio_detailed_connectivity_release(PioDetailedConnectivity *details);

size_t pio_balanced_network_bus_count(const PioBalancedNetwork *network);

size_t pio_balanced_network_branch_count(const PioBalancedNetwork *network);

size_t pio_balanced_network_load_count(const PioBalancedNetwork *network);

size_t pio_balanced_network_shunt_count(const PioBalancedNetwork *network);

size_t pio_balanced_network_static_var_compensator_count(const PioBalancedNetwork *network);

size_t pio_balanced_network_generator_count(const PioBalancedNetwork *network);

size_t pio_balanced_network_storage_count(const PioBalancedNetwork *network);

size_t pio_balanced_network_switch_count(const PioBalancedNetwork *network);

size_t pio_balanced_network_hvdc_count(const PioBalancedNetwork *network);

size_t pio_balanced_network_three_winding_transformer_count(const PioBalancedNetwork *network);

size_t pio_balanced_network_area_count(const PioBalancedNetwork *network);

/**
 * Read one bus by zero based table position.
 */
bool pio_balanced_network_bus_at(const PioBalancedNetwork *network,
                                 size_t index,
                                 PioBalancedBusView *output,
                                 PioError **error);

/**
 * Read one load by zero based table position.
 */
bool pio_balanced_network_load_at(const PioBalancedNetwork *network,
                                  size_t index,
                                  PioBalancedLoadView *output,
                                  PioError **error);

/**
 * Read one shunt by zero based table position.
 */
bool pio_balanced_network_shunt_at(const PioBalancedNetwork *network,
                                   size_t index,
                                   PioBalancedShuntView *output,
                                   PioError **error);

/**
 * Read one switched shunt block by zero based position.
 */
bool pio_balanced_network_shunt_block_at(const PioBalancedNetwork *network,
                                         size_t shunt_index,
                                         size_t block_index,
                                         PioShuntBlockView *output,
                                         PioError **error);

/**
 * Read one static VAR compensator by zero based table position.
 */
bool pio_balanced_network_static_var_compensator_at(const PioBalancedNetwork *network,
                                                    size_t index,
                                                    PioBalancedStaticVarCompensatorView *output,
                                                    PioError **error);

/**
 * Read one branch by zero based table position.
 */
bool pio_balanced_network_branch_at(const PioBalancedNetwork *network,
                                    size_t index,
                                    PioBalancedBranchView *output,
                                    PioError **error);

/**
 * Read one point from an explicitly stored balanced branch route.
 */
bool pio_balanced_network_branch_route_point_at(const PioBalancedNetwork *network,
                                                size_t branch_index,
                                                size_t point_index,
                                                PioBalancedLocationView *output,
                                                PioError **error);

/**
 * Read one additional named branch MVA rating.
 */
bool pio_balanced_network_branch_rating_at(const PioBalancedNetwork *network,
                                           size_t branch_index,
                                           size_t rating_index,
                                           PioBranchRatingView *output,
                                           PioError **error);

/**
 * Read one generator by zero based table position.
 */
bool pio_balanced_network_generator_at(const PioBalancedNetwork *network,
                                       size_t index,
                                       PioBalancedGeneratorView *output,
                                       PioError **error);

/**
 * Read one named generator capability or ramp field.
 */
bool pio_balanced_network_generator_capability_at(const PioBalancedNetwork *network,
                                                  size_t generator_index,
                                                  size_t capability_index,
                                                  PioGeneratorCapabilityView *output,
                                                  PioError **error);

/**
 * Read one storage element by zero based table position.
 */
bool pio_balanced_network_storage_at(const PioBalancedNetwork *network,
                                     size_t index,
                                     PioBalancedStorageView *output,
                                     PioError **error);

/**
 * Read one transmission switch by zero based table position.
 */
bool pio_balanced_network_switch_at(const PioBalancedNetwork *network,
                                    size_t index,
                                    PioBalancedSwitchView *output,
                                    PioError **error);

/**
 * Read one HVDC line by zero based table position.
 */
bool pio_balanced_network_hvdc_at(const PioBalancedNetwork *network,
                                  size_t index,
                                  PioBalancedHvdcView *output,
                                  PioError **error);

/**
 * Read one three winding transformer by zero based table position.
 */
bool pio_balanced_network_three_winding_transformer_at(const PioBalancedNetwork *network,
                                                       size_t index,
                                                       PioBalancedThreeWindingTransformerView *output,
                                                       PioError **error);

/**
 * Read one winding of a three winding transformer.
 */
bool pio_balanced_network_three_winding_transformer_winding_at(const PioBalancedNetwork *network,
                                                               size_t transformer_index,
                                                               size_t winding_index,
                                                               PioThreeWindingTransformerWindingView *output,
                                                               PioError **error);

/**
 * Read one pairwise impedance of a three winding transformer.
 */
bool pio_balanced_network_three_winding_transformer_impedance_at(const PioBalancedNetwork *network,
                                                                 size_t transformer_index,
                                                                 size_t impedance_index,
                                                                 PioThreeWindingTransformerImpedanceView *output,
                                                                 PioError **error);

/**
 * Read one control area by zero based table position.
 */
bool pio_balanced_network_area_at(const PioBalancedNetwork *network,
                                  size_t index,
                                  PioBalancedAreaView *output,
                                  PioError **error);

PioBalancedNetwork *pio_balanced_network_retain(const PioBalancedNetwork *network);

void pio_balanced_network_release(PioBalancedNetwork *network);

PioStringView pio_multiconductor_network_name(const PioMulticonductorNetwork *network);

bool pio_multiconductor_network_has_name(const PioMulticonductorNetwork *network);

PioStringView pio_multiconductor_network_source_format(const PioMulticonductorNetwork *network);

bool pio_multiconductor_network_has_source_format(const PioMulticonductorNetwork *network);

/**
 * Read the network coordinate metadata, including absence through `has_geo`.
 */
bool pio_multiconductor_network_geo(const PioMulticonductorNetwork *network,
                                    PioMulticonductorGeoView *output,
                                    PioError **error);

/**
 * Read exact table lengths. Defaulted source fields and arbitrary extension
 * maps are retained internally and are not separate domain tables.
 */
bool pio_multiconductor_network_counts(const PioMulticonductorNetwork *network,
                                       PioMulticonductorNetworkCountsView *output,
                                       PioError **error);

double pio_multiconductor_network_base_frequency_hz(const PioMulticonductorNetwork *network);

size_t pio_multiconductor_network_bus_count(const PioMulticonductorNetwork *network);

size_t pio_multiconductor_network_line_count(const PioMulticonductorNetwork *network);

size_t pio_multiconductor_network_load_count(const PioMulticonductorNetwork *network);

size_t pio_multiconductor_network_generator_count(const PioMulticonductorNetwork *network);

/**
 * Read one multiconductor bus by zero based table position. Borrowed strings
 * and numeric spans remain valid while the network handle is alive.
 */
bool pio_multiconductor_network_bus_at(const PioMulticonductorNetwork *network,
                                       size_t index,
                                       PioMulticonductorBusView *output,
                                       PioError **error);

bool pio_multiconductor_network_bus_terminal_at(const PioMulticonductorNetwork *network,
                                                size_t bus_index,
                                                size_t terminal_index,
                                                PioStringView *output,
                                                PioError **error);

bool pio_multiconductor_network_bus_grounded_terminal_at(const PioMulticonductorNetwork *network,
                                                         size_t bus_index,
                                                         size_t terminal_index,
                                                         PioStringView *output,
                                                         PioError **error);

bool pio_multiconductor_network_line_code_at(const PioMulticonductorNetwork *network,
                                             size_t index,
                                             PioMulticonductorLineCodeView *output,
                                             PioError **error);

bool pio_multiconductor_network_line_code_resistance_matrix_row_at(const PioMulticonductorNetwork *network,
                                                                   size_t line_code_index,
                                                                   size_t row_index,
                                                                   PioF64View *output,
                                                                   PioError **error);

bool pio_multiconductor_network_line_code_reactance_matrix_row_at(const PioMulticonductorNetwork *network,
                                                                  size_t line_code_index,
                                                                  size_t row_index,
                                                                  PioF64View *output,
                                                                  PioError **error);

bool pio_multiconductor_network_line_code_conductance_from_matrix_row_at(const PioMulticonductorNetwork *network,
                                                                         size_t line_code_index,
                                                                         size_t row_index,
                                                                         PioF64View *output,
                                                                         PioError **error);

bool pio_multiconductor_network_line_code_susceptance_from_matrix_row_at(const PioMulticonductorNetwork *network,
                                                                         size_t line_code_index,
                                                                         size_t row_index,
                                                                         PioF64View *output,
                                                                         PioError **error);

bool pio_multiconductor_network_line_code_conductance_to_matrix_row_at(const PioMulticonductorNetwork *network,
                                                                       size_t line_code_index,
                                                                       size_t row_index,
                                                                       PioF64View *output,
                                                                       PioError **error);

bool pio_multiconductor_network_line_code_susceptance_to_matrix_row_at(const PioMulticonductorNetwork *network,
                                                                       size_t line_code_index,
                                                                       size_t row_index,
                                                                       PioF64View *output,
                                                                       PioError **error);

bool pio_multiconductor_network_line_at(const PioMulticonductorNetwork *network,
                                        size_t index,
                                        PioMulticonductorLineView *output,
                                        PioError **error);

bool pio_multiconductor_network_line_terminal_from_at(const PioMulticonductorNetwork *network,
                                                      size_t line_index,
                                                      size_t terminal_index,
                                                      PioStringView *output,
                                                      PioError **error);

bool pio_multiconductor_network_line_terminal_to_at(const PioMulticonductorNetwork *network,
                                                    size_t line_index,
                                                    size_t terminal_index,
                                                    PioStringView *output,
                                                    PioError **error);

bool pio_multiconductor_network_line_route_point_at(const PioMulticonductorNetwork *network,
                                                    size_t line_index,
                                                    size_t point_index,
                                                    PioMulticonductorLocationView *output,
                                                    PioError **error);

bool pio_multiconductor_network_switch_at(const PioMulticonductorNetwork *network,
                                          size_t index,
                                          PioMulticonductorSwitchView *output,
                                          PioError **error);

bool pio_multiconductor_network_switch_terminal_from_at(const PioMulticonductorNetwork *network,
                                                        size_t switch_index,
                                                        size_t terminal_index,
                                                        PioStringView *output,
                                                        PioError **error);

bool pio_multiconductor_network_switch_terminal_to_at(const PioMulticonductorNetwork *network,
                                                      size_t switch_index,
                                                      size_t terminal_index,
                                                      PioStringView *output,
                                                      PioError **error);

bool pio_multiconductor_network_transformer_at(const PioMulticonductorNetwork *network,
                                               size_t index,
                                               PioMulticonductorTransformerView *output,
                                               PioError **error);

bool pio_multiconductor_network_transformer_winding_at(const PioMulticonductorNetwork *network,
                                                       size_t transformer_index,
                                                       size_t winding_index,
                                                       PioMulticonductorTransformerWindingView *output,
                                                       PioError **error);

bool pio_multiconductor_network_transformer_winding_terminal_at(const PioMulticonductorNetwork *network,
                                                                size_t transformer_index,
                                                                size_t winding_index,
                                                                size_t terminal_index,
                                                                PioStringView *output,
                                                                PioError **error);

bool pio_multiconductor_network_load_at(const PioMulticonductorNetwork *network,
                                        size_t index,
                                        PioMulticonductorLoadView *output,
                                        PioError **error);

bool pio_multiconductor_network_load_terminal_at(const PioMulticonductorNetwork *network,
                                                 size_t load_index,
                                                 size_t terminal_index,
                                                 PioStringView *output,
                                                 PioError **error);

bool pio_multiconductor_network_generator_at(const PioMulticonductorNetwork *network,
                                             size_t index,
                                             PioMulticonductorGeneratorView *output,
                                             PioError **error);

bool pio_multiconductor_network_generator_terminal_at(const PioMulticonductorNetwork *network,
                                                      size_t generator_index,
                                                      size_t terminal_index,
                                                      PioStringView *output,
                                                      PioError **error);

bool pio_multiconductor_network_inverter_based_resource_at(const PioMulticonductorNetwork *network,
                                                           size_t index,
                                                           PioInverterBasedResourceView *output,
                                                           PioError **error);

bool pio_multiconductor_network_inverter_based_resource_terminal_at(const PioMulticonductorNetwork *network,
                                                                    size_t resource_index,
                                                                    size_t terminal_index,
                                                                    PioStringView *output,
                                                                    PioError **error);

bool pio_multiconductor_network_control_profile_at(const PioMulticonductorNetwork *network,
                                                   size_t index,
                                                   PioControlProfileView *output,
                                                   PioError **error);

bool pio_multiconductor_network_shunt_at(const PioMulticonductorNetwork *network,
                                         size_t index,
                                         PioMulticonductorShuntView *output,
                                         PioError **error);

bool pio_multiconductor_network_shunt_terminal_at(const PioMulticonductorNetwork *network,
                                                  size_t shunt_index,
                                                  size_t terminal_index,
                                                  PioStringView *output,
                                                  PioError **error);

bool pio_multiconductor_network_shunt_conductance_matrix_row_at(const PioMulticonductorNetwork *network,
                                                                size_t shunt_index,
                                                                size_t row_index,
                                                                PioF64View *output,
                                                                PioError **error);

bool pio_multiconductor_network_shunt_susceptance_matrix_row_at(const PioMulticonductorNetwork *network,
                                                                size_t shunt_index,
                                                                size_t row_index,
                                                                PioF64View *output,
                                                                PioError **error);

bool pio_multiconductor_network_capacitor_at(const PioMulticonductorNetwork *network,
                                             size_t index,
                                             PioMulticonductorCapacitorView *output,
                                             PioError **error);

bool pio_multiconductor_network_capacitor_terminal_at(const PioMulticonductorNetwork *network,
                                                      size_t capacitor_index,
                                                      size_t terminal_index,
                                                      PioStringView *output,
                                                      PioError **error);

bool pio_multiconductor_network_voltage_source_at(const PioMulticonductorNetwork *network,
                                                  size_t index,
                                                  PioVoltageSourceView *output,
                                                  PioError **error);

bool pio_multiconductor_network_voltage_source_terminal_at(const PioMulticonductorNetwork *network,
                                                           size_t source_index,
                                                           size_t terminal_index,
                                                           PioStringView *output,
                                                           PioError **error);

bool pio_multiconductor_network_untyped_object_at(const PioMulticonductorNetwork *network,
                                                  size_t index,
                                                  PioMulticonductorUntypedObjectView *output,
                                                  PioError **error);

bool pio_multiconductor_network_untyped_object_property_at(const PioMulticonductorNetwork *network,
                                                           size_t object_index,
                                                           size_t property_index,
                                                           PioMulticonductorUntypedPropertyView *output,
                                                           PioError **error);

bool pio_multiconductor_network_command_at(const PioMulticonductorNetwork *network,
                                           size_t index,
                                           PioMulticonductorCommandView *output,
                                           PioError **error);

bool pio_multiconductor_network_option_at(const PioMulticonductorNetwork *network,
                                          size_t index,
                                          PioStringPropertyView *output,
                                          PioError **error);

PioMulticonductorNetwork *pio_multiconductor_network_retain(const PioMulticonductorNetwork *network);

void pio_multiconductor_network_release(PioMulticonductorNetwork *network);

/**
 * Construct a stable component identity.
 */
PioComponentId *pio_component_id_new(const char *component_type,
                                     size_t component_type_len,
                                     const char *local_id,
                                     size_t local_id_len,
                                     PioError **error);

PioStringView pio_component_id_type(const PioComponentId *component);

PioStringView pio_component_id_local_id(const PioComponentId *component);

PioComponentId *pio_component_id_retain(const PioComponentId *component);

void pio_component_id_release(PioComponentId *component);

PioActivePower *pio_active_power_from_watts(double value);

PioActivePower *pio_active_power_from_megawatts(double value);

double pio_active_power_value(const PioActivePower *power);

PioStringView pio_active_power_unit(const PioActivePower *power);

PioActivePower *pio_active_power_retain(const PioActivePower *power);

void pio_active_power_release(PioActivePower *power);

PioReactivePower *pio_reactive_power_from_vars(double value);

PioReactivePower *pio_reactive_power_from_megavars(double value);

double pio_reactive_power_value(const PioReactivePower *power);

PioStringView pio_reactive_power_unit(const PioReactivePower *power);

PioReactivePower *pio_reactive_power_retain(const PioReactivePower *power);

void pio_reactive_power_release(PioReactivePower *power);

PioApparentPower *pio_apparent_power_from_volt_amperes(double value);

PioApparentPower *pio_apparent_power_from_megavolt_amperes(double value);

double pio_apparent_power_value(const PioApparentPower *power);

PioStringView pio_apparent_power_unit(const PioApparentPower *power);

PioApparentPower *pio_apparent_power_retain(const PioApparentPower *power);

void pio_apparent_power_release(PioApparentPower *power);

PioOperatingPointUpdate *pio_operating_point_update_set_load_active_power(const PioComponentId *load,
                                                                          const char *terminal,
                                                                          size_t terminal_len,
                                                                          const PioActivePower *power,
                                                                          PioError **error);

PioOperatingPointUpdate *pio_operating_point_update_set_load_reactive_power(const PioComponentId *load,
                                                                            const char *terminal,
                                                                            size_t terminal_len,
                                                                            const PioReactivePower *power,
                                                                            PioError **error);

PioOperatingPointUpdate *pio_operating_point_update_set_generator_active_power(const PioComponentId *generator,
                                                                               const char *terminal,
                                                                               size_t terminal_len,
                                                                               const PioActivePower *power,
                                                                               PioError **error);

PioOperatingPointUpdate *pio_operating_point_update_set_generator_reactive_power(const PioComponentId *generator,
                                                                                 const char *terminal,
                                                                                 size_t terminal_len,
                                                                                 const PioReactivePower *power,
                                                                                 PioError **error);

PioOperatingPointUpdate *pio_operating_point_update_set_generator_voltage_magnitude(const PioComponentId *generator,
                                                                                    double voltage_magnitude_per_unit,
                                                                                    PioError **error);

PioOperatingPointUpdate *pio_operating_point_update_set_generator_in_service(const PioComponentId *generator,
                                                                             bool in_service,
                                                                             PioError **error);

PioOperatingPointUpdate *pio_operating_point_update_set_branch_in_service(const PioComponentId *branch,
                                                                          bool in_service,
                                                                          PioError **error);

PioOperatingPointUpdate *pio_operating_point_update_set_transformer_tap_ratio(const PioComponentId *transformer,
                                                                              double tap_ratio,
                                                                              PioError **error);

PioOperatingPointUpdate *pio_operating_point_update_set_transformer_phase_shift_degrees(const PioComponentId *transformer,
                                                                                        double phase_shift_degrees,
                                                                                        PioError **error);

PioOperatingPointUpdate *pio_operating_point_update_set_switch_closed(const PioComponentId *switch_id,
                                                                      bool closed,
                                                                      PioError **error);

PioNetworkUpdate *pio_network_update_set_branch_thermal_rating(const PioComponentId *branch,
                                                               const char *terminal,
                                                               size_t terminal_len,
                                                               const PioApparentPower *rating,
                                                               PioError **error);

PioCalculationUpdate *pio_calculation_update_from_operating_point(const PioOperatingPointUpdate *update,
                                                                  PioError **error);

PioCalculationUpdate *pio_calculation_update_from_network(const PioNetworkUpdate *update,
                                                          PioError **error);

PioOperatingPointUpdate *pio_operating_point_update_retain(const PioOperatingPointUpdate *update);

void pio_operating_point_update_release(PioOperatingPointUpdate *update);

PioNetworkUpdate *pio_network_update_retain(const PioNetworkUpdate *update);

void pio_network_update_release(PioNetworkUpdate *update);

PioCalculationUpdate *pio_calculation_update_retain(const PioCalculationUpdate *update);

void pio_calculation_update_release(PioCalculationUpdate *update);

/**
 * Apply a complete typed update batch atomically.
 *
 * Owner rooted handles obtained before the call (values, networks, collection
 * entries, artifacts) keep the pre-update module alive. Plain view structs read
 * from this module handle (`PioStringView`, `PioModuleSourceView`, history and
 * source map views) are invalidated by a successful call and must be read
 * again. The caller must hold exclusive access to `module` for the duration of
 * the call: no concurrent call of any kind on this handle, including retain.
 */
PioUpdateReport *pio_apply_updates(PioModule *module,
                                   const PioCalculationUpdate *const *updates,
                                   size_t updates_len,
                                   PioError **error);

/**
 * Replace aggregate active demand at one bus using the named allocation rule.
 *
 * The same view invalidation and exclusivity rules as `pio_apply_updates`
 * apply.
 */
PioUpdateReport *pio_apply_bus_load_active_power(PioModule *module,
                                                 size_t bus_id,
                                                 const PioActivePower *power,
                                                 const char *allocation,
                                                 size_t allocation_len,
                                                 PioError **error);

size_t pio_update_report_len(const PioUpdateReport *report);

bool pio_update_report_connectivity_changed(const PioUpdateReport *report);

PioUpdateChange *pio_update_report_change(const PioUpdateReport *report,
                                          size_t index,
                                          PioError **error);

PioComponentId *pio_update_change_component_id(const PioUpdateChange *change);

PioStringView pio_update_change_field(const PioUpdateChange *change);

PioStringView pio_update_change_terminal(const PioUpdateChange *change);

PioUpdateReport *pio_update_report_retain(const PioUpdateReport *report);

void pio_update_report_release(PioUpdateReport *report);

PioUpdateChange *pio_update_change_retain(const PioUpdateChange *change);

void pio_update_change_release(PioUpdateChange *change);

/**
 * Emit one module as a grid exchange format.
 */
PioEmitResult *pio_emit(const PioModule *module,
                        const char *format,
                        size_t format_len,
                        const PioDestination *destination,
                        PioError **error);

/**
 * Serialize one module as PowerIO IR.
 */
PioEmitResult *pio_module_serialize(const PioModule *module,
                                    const PioDestination *destination,
                                    PioError **error);

PioStringView pio_emit_result_layout(const PioEmitResult *result);

PioStringView pio_emit_result_fidelity(const PioEmitResult *result);

size_t pio_emit_result_artifact_count(const PioEmitResult *result);

PioArtifact *pio_emit_result_artifact(const PioEmitResult *result, size_t index, PioError **error);

PioDiagnostics *pio_emit_result_diagnostics(const PioEmitResult *result);

PioEmitResult *pio_emit_result_retain(const PioEmitResult *result);

void pio_emit_result_release(PioEmitResult *result);

PioStringView pio_artifact_name(const PioArtifact *artifact);

/**
 * Return emitted memory bytes. A path destination has no memory bytes and
 * returns an empty view.
 */
PioByteView pio_artifact_bytes(const PioArtifact *artifact);

PioArtifact *pio_artifact_retain(const PioArtifact *artifact);

void pio_artifact_release(PioArtifact *artifact);

PioStringView pio_calculation_solution_termination(const PioCalculationSolution *solution);

/**
 * Return an OPF or SCUC objective. SOCWR reports a lower bound through
 * pio_socwr_opf_solution_get_objective_lower_bound instead.
 */
bool pio_calculation_solution_get_objective(const PioCalculationSolution *solution,
                                            double *out_objective);

bool pio_socwr_opf_solution_get_objective_lower_bound(const PioCalculationSolution *solution,
                                                      double *out_lower_bound);

/**
 * Copy one named solution quantity into an independently owned vector.
 */
PioVector *pio_calculation_solution_get_values(const PioCalculationSolution *solution,
                                               const char *quantity,
                                               size_t quantity_len,
                                               PioError **error);

size_t pio_ac_scuc_solution_time_count(const PioCalculationSolution *solution);

/**
 * Copy one AC SCUC output row for one time position into an owned vector.
 */
PioVector *pio_ac_scuc_solution_get_values_at(const PioCalculationSolution *solution,
                                              const char *quantity,
                                              size_t quantity_len,
                                              size_t time_index,
                                              PioError **error);

PioSparseMatrix *pio_calc_incidence_matrix(const PioBalancedNetwork *network,
                                           const char *formula,
                                           size_t formula_len,
                                           PioError **error);

PioSparseMatrix *pio_calc_bus_susceptance_matrix(const PioBalancedNetwork *network,
                                                 const char *formula,
                                                 size_t formula_len,
                                                 PioError **error);

PioSparseMatrix *pio_calc_branch_flow_matrix(const PioBalancedNetwork *network,
                                             const char *formula,
                                             size_t formula_len,
                                             PioError **error);

PioVector *pio_calc_branch_susceptances(const PioBalancedNetwork *network,
                                        const char *formula,
                                        size_t formula_len,
                                        PioError **error);

PioVector *pio_calc_branch_phase_shift_injection(const PioBalancedNetwork *network,
                                                 const char *formula,
                                                 size_t formula_len,
                                                 PioError **error);

PioVector *pio_calc_bus_phase_shift_injection(const PioBalancedNetwork *network,
                                              const char *formula,
                                              size_t formula_len,
                                              PioError **error);

PioVector *pio_calc_branch_flow_dc(const PioBalancedNetwork *network,
                                   const char *formula,
                                   size_t formula_len,
                                   const double *voltage_angles,
                                   size_t voltage_angles_len,
                                   PioError **error);

PioVector *pio_calc_bus_injection_dc(const PioBalancedNetwork *network,
                                     const char *formula,
                                     size_t formula_len,
                                     const double *voltage_angles,
                                     size_t voltage_angles_len,
                                     PioError **error);

size_t pio_sparse_matrix_rows(const PioSparseMatrix *matrix);

size_t pio_sparse_matrix_columns(const PioSparseMatrix *matrix);

PioSizeView pio_sparse_matrix_row_offsets(const PioSparseMatrix *matrix);

PioSizeView pio_sparse_matrix_column_indices(const PioSparseMatrix *matrix);

PioF64View pio_sparse_matrix_values(const PioSparseMatrix *matrix);

PioSparseMatrix *pio_sparse_matrix_retain(const PioSparseMatrix *matrix);

void pio_sparse_matrix_release(PioSparseMatrix *matrix);

PioF64View pio_vector_values(const PioVector *vector);

PioVector *pio_vector_retain(const PioVector *vector);

void pio_vector_release(PioVector *vector);

/**
 * Return version information for this ABI and the PowerIO IR serializer/deserializer.
 */
PioString *pio_schema_report(PioError **error);

PioStringView pio_string_view(const PioString *string);

PioString *pio_string_retain(const PioString *string);

void pio_string_release(PioString *string);

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus

#endif  /* POWERIO_H */
