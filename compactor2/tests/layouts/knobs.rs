//! Module for testing / demonstrating how configuration parameters
//! (knobs) affect the compactor output.
//!
//! See [crate::layout] module for detailed documentation

use crate::layouts::{all_overlapping_l0_files, layout_setup_builder, run_layout_scenario, ONE_MB};

/// This test show the effects of the split_percentage parameter
#[tokio::test]
async fn all_overlapping_l0_split_percentage() {
    test_helpers::maybe_start_logging();

    //
    // configure to leave a smaller part of  data on the leading
    // edge.
    //
    // Expect to see two output files, one having the oldest 95
    // percent of the data, and one having the newest 5 percent.
    let setup = layout_setup_builder()
        .await
        .with_split_percentage(95)
        .build()
        .await;

    // create virtual files
    let setup = all_overlapping_l0_files(setup).await;

    insta::assert_yaml_snapshot!(
        run_layout_scenario(&setup).await,
        @r###"
    ---
    - "**** Input Files "
    - "L0, all files 9mb                                                                                   "
    - "L0.1[100,200]       |-------------------------------------L0.1-------------------------------------|"
    - "L0.2[100,200]       |-------------------------------------L0.2-------------------------------------|"
    - "L0.3[100,200]       |-------------------------------------L0.3-------------------------------------|"
    - "L0.4[100,200]       |-------------------------------------L0.4-------------------------------------|"
    - "L0.5[100,200]       |-------------------------------------L0.5-------------------------------------|"
    - "L0.6[100,200]       |-------------------------------------L0.6-------------------------------------|"
    - "L0.7[100,200]       |-------------------------------------L0.7-------------------------------------|"
    - "L0.8[100,200]       |-------------------------------------L0.8-------------------------------------|"
    - "L0.9[100,200]       |-------------------------------------L0.9-------------------------------------|"
    - "L0.10[100,200]      |------------------------------------L0.10-------------------------------------|"
    - "**** Simulation run 0, type=split(split_times=[195]). 10 Input Files, 90mb total:"
    - "L0, all files 9mb                                                                                   "
    - "L0.10[100,200]      |------------------------------------L0.10-------------------------------------|"
    - "L0.9[100,200]       |-------------------------------------L0.9-------------------------------------|"
    - "L0.8[100,200]       |-------------------------------------L0.8-------------------------------------|"
    - "L0.7[100,200]       |-------------------------------------L0.7-------------------------------------|"
    - "L0.6[100,200]       |-------------------------------------L0.6-------------------------------------|"
    - "L0.5[100,200]       |-------------------------------------L0.5-------------------------------------|"
    - "L0.4[100,200]       |-------------------------------------L0.4-------------------------------------|"
    - "L0.3[100,200]       |-------------------------------------L0.3-------------------------------------|"
    - "L0.2[100,200]       |-------------------------------------L0.2-------------------------------------|"
    - "L0.1[100,200]       |-------------------------------------L0.1-------------------------------------|"
    - "**** 2 Output Files (parquet_file_id not yet assigned), 90mb total:"
    - "L1                                                                                                  "
    - "L1.?[100,195] 85.5mb|-----------------------------------L1.?-----------------------------------|    "
    - "L1.?[195,200] 4.5mb                                                                             |L1.?|"
    - "**** Final Output Files "
    - "L1                                                                                                  "
    - "L1.11[100,195] 85.5mb|----------------------------------L1.11-----------------------------------|    "
    - "L1.12[195,200] 4.5mb                                                                            |L1.12|"
    "###
    );
}

/// This test show the effects of the percentage_max_file_size parameter
#[tokio::test]
async fn all_overlapping_l0_max_file_size() {
    test_helpers::maybe_start_logging();

    //
    // configure the max file size percentage to be 95%
    //
    // This means that the compactor will only apply the "leading
    // edge" optimization for data that is greater than 95 * 100MB =
    // 95MB in total
    //
    // Expect to see a single output file
    let setup = layout_setup_builder()
        .await
        .with_percentage_max_file_size(95)
        .build()
        .await;

    // create virtual files
    let setup = all_overlapping_l0_files(setup).await;

    insta::assert_yaml_snapshot!(
        run_layout_scenario(&setup).await,
        @r###"
    ---
    - "**** Input Files "
    - "L0, all files 9mb                                                                                   "
    - "L0.1[100,200]       |-------------------------------------L0.1-------------------------------------|"
    - "L0.2[100,200]       |-------------------------------------L0.2-------------------------------------|"
    - "L0.3[100,200]       |-------------------------------------L0.3-------------------------------------|"
    - "L0.4[100,200]       |-------------------------------------L0.4-------------------------------------|"
    - "L0.5[100,200]       |-------------------------------------L0.5-------------------------------------|"
    - "L0.6[100,200]       |-------------------------------------L0.6-------------------------------------|"
    - "L0.7[100,200]       |-------------------------------------L0.7-------------------------------------|"
    - "L0.8[100,200]       |-------------------------------------L0.8-------------------------------------|"
    - "L0.9[100,200]       |-------------------------------------L0.9-------------------------------------|"
    - "L0.10[100,200]      |------------------------------------L0.10-------------------------------------|"
    - "**** Simulation run 0, type=compact. 10 Input Files, 90mb total:"
    - "L0, all files 9mb                                                                                   "
    - "L0.10[100,200]      |------------------------------------L0.10-------------------------------------|"
    - "L0.9[100,200]       |-------------------------------------L0.9-------------------------------------|"
    - "L0.8[100,200]       |-------------------------------------L0.8-------------------------------------|"
    - "L0.7[100,200]       |-------------------------------------L0.7-------------------------------------|"
    - "L0.6[100,200]       |-------------------------------------L0.6-------------------------------------|"
    - "L0.5[100,200]       |-------------------------------------L0.5-------------------------------------|"
    - "L0.4[100,200]       |-------------------------------------L0.4-------------------------------------|"
    - "L0.3[100,200]       |-------------------------------------L0.3-------------------------------------|"
    - "L0.2[100,200]       |-------------------------------------L0.2-------------------------------------|"
    - "L0.1[100,200]       |-------------------------------------L0.1-------------------------------------|"
    - "**** 1 Output Files (parquet_file_id not yet assigned), 90mb total:"
    - "L1, all files 90mb                                                                                  "
    - "L1.?[100,200]       |-------------------------------------L1.?-------------------------------------|"
    - "**** Final Output Files "
    - "L1, all files 90mb                                                                                  "
    - "L1.11[100,200]      |------------------------------------L1.11-------------------------------------|"
    "###
    );
}

/// This test show how the split percentage and max_file size ineract
#[tokio::test]
async fn all_overlapping_l0_split_percentage_and_max_file_size() {
    test_helpers::maybe_start_logging();

    // configure the max file size percentage to be 95% and the max file size to be 10MB
    //
    // This means that the compactor will only apply the "leading
    // edge" optimization for data that is greater than 95 * 10MB =
    // 9.5 MB in total
    //
    // Expect to see 8 ~11mb files, 1 file that s smaller
    let setup = layout_setup_builder()
        .await
        .with_max_desired_file_size_bytes(10 * ONE_MB)
        .with_percentage_max_file_size(95)
        .build()
        .await;

    // create virtual files
    let setup = all_overlapping_l0_files(setup).await;

    insta::assert_yaml_snapshot!(
        run_layout_scenario(&setup).await,
        @r###"
    ---
    - "**** Input Files "
    - "L0, all files 9mb                                                                                   "
    - "L0.1[100,200]       |-------------------------------------L0.1-------------------------------------|"
    - "L0.2[100,200]       |-------------------------------------L0.2-------------------------------------|"
    - "L0.3[100,200]       |-------------------------------------L0.3-------------------------------------|"
    - "L0.4[100,200]       |-------------------------------------L0.4-------------------------------------|"
    - "L0.5[100,200]       |-------------------------------------L0.5-------------------------------------|"
    - "L0.6[100,200]       |-------------------------------------L0.6-------------------------------------|"
    - "L0.7[100,200]       |-------------------------------------L0.7-------------------------------------|"
    - "L0.8[100,200]       |-------------------------------------L0.8-------------------------------------|"
    - "L0.9[100,200]       |-------------------------------------L0.9-------------------------------------|"
    - "L0.10[100,200]      |------------------------------------L0.10-------------------------------------|"
    - "**** Simulation run 0, type=split(split_times=[112, 124, 136, 148, 160, 172, 184, 196]). 10 Input Files, 90mb total:"
    - "L0, all files 9mb                                                                                   "
    - "L0.10[100,200]      |------------------------------------L0.10-------------------------------------|"
    - "L0.9[100,200]       |-------------------------------------L0.9-------------------------------------|"
    - "L0.8[100,200]       |-------------------------------------L0.8-------------------------------------|"
    - "L0.7[100,200]       |-------------------------------------L0.7-------------------------------------|"
    - "L0.6[100,200]       |-------------------------------------L0.6-------------------------------------|"
    - "L0.5[100,200]       |-------------------------------------L0.5-------------------------------------|"
    - "L0.4[100,200]       |-------------------------------------L0.4-------------------------------------|"
    - "L0.3[100,200]       |-------------------------------------L0.3-------------------------------------|"
    - "L0.2[100,200]       |-------------------------------------L0.2-------------------------------------|"
    - "L0.1[100,200]       |-------------------------------------L0.1-------------------------------------|"
    - "**** 9 Output Files (parquet_file_id not yet assigned), 90mb total:"
    - "L1                                                                                                  "
    - "L1.?[100,112] 10.8mb|-L1.?--|                                                                       "
    - "L1.?[112,124] 10.8mb         |-L1.?--|                                                              "
    - "L1.?[124,136] 10.8mb                   |-L1.?--|                                                    "
    - "L1.?[136,148] 10.8mb                            |-L1.?--|                                           "
    - "L1.?[148,160] 10.8mb                                      |-L1.?--|                                 "
    - "L1.?[160,172] 10.8mb                                                |-L1.?--|                       "
    - "L1.?[172,184] 10.8mb                                                         |-L1.?--|              "
    - "L1.?[184,196] 10.8mb                                                                   |-L1.?--|    "
    - "L1.?[196,200] 3.6mb                                                                             |L1.?|"
    - "**** Simulation run 1, type=split(split_times=[112, 124, 136, 148, 160, 172, 184, 196]). 9 Input Files, 90mb total:"
    - "L1                                                                                                  "
    - "L1.19[196,200] 3.6mb                                                                            |L1.19|"
    - "L1.18[184,196] 10.8mb                                                                   |-L1.18-|    "
    - "L1.17[172,184] 10.8mb                                                         |-L1.17-|              "
    - "L1.16[160,172] 10.8mb                                                |-L1.16-|                       "
    - "L1.15[148,160] 10.8mb                                      |-L1.15-|                                 "
    - "L1.14[136,148] 10.8mb                            |-L1.14-|                                           "
    - "L1.13[124,136] 10.8mb                   |-L1.13-|                                                    "
    - "L1.12[112,124] 10.8mb         |-L1.12-|                                                              "
    - "L1.11[100,112] 10.8mb|-L1.11-|                                                                       "
    - "**** 9 Output Files (parquet_file_id not yet assigned), 90mb total:"
    - "L2                                                                                                  "
    - "L2.?[100,112] 10.8mb|-L2.?--|                                                                       "
    - "L2.?[112,124] 10.8mb         |-L2.?--|                                                              "
    - "L2.?[124,136] 10.8mb                   |-L2.?--|                                                    "
    - "L2.?[136,148] 10.8mb                            |-L2.?--|                                           "
    - "L2.?[148,160] 10.8mb                                      |-L2.?--|                                 "
    - "L2.?[160,172] 10.8mb                                                |-L2.?--|                       "
    - "L2.?[172,184] 10.8mb                                                         |-L2.?--|              "
    - "L2.?[184,196] 10.8mb                                                                   |-L2.?--|    "
    - "L2.?[196,200] 3.6mb                                                                             |L2.?|"
    - "**** Final Output Files "
    - "L2                                                                                                  "
    - "L2.20[100,112] 10.8mb|-L2.20-|                                                                       "
    - "L2.21[112,124] 10.8mb         |-L2.21-|                                                              "
    - "L2.22[124,136] 10.8mb                   |-L2.22-|                                                    "
    - "L2.23[136,148] 10.8mb                            |-L2.23-|                                           "
    - "L2.24[148,160] 10.8mb                                      |-L2.24-|                                 "
    - "L2.25[160,172] 10.8mb                                                |-L2.25-|                       "
    - "L2.26[172,184] 10.8mb                                                         |-L2.26-|              "
    - "L2.27[184,196] 10.8mb                                                                   |-L2.27-|    "
    - "L2.28[196,200] 3.6mb                                                                            |L2.28|"
    "###
    );
}