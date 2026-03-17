// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @notice Simulates Ethereum transaction execution by replaying each tx's
///         read/write set until a target access count is reached.
///
/// Access count = gasUsed / 100, where 100 is the cost of one hot SLOAD —
/// the cheapest storage operation. This converts gas into a number of
/// storage accesses that approximates the original tx's workload.
///
/// The caller must provide a large enough gas budget so the simulation
/// never runs out of gas (e.g. gasUsed * 300 is sufficient since most
/// accesses after the first round are hot at 100 gas each).
///
/// Conflicts emerge naturally: two txs conflict if they share a key in
/// their write sets, or one reads what the other writes, because they
/// both access the same slot in the shared `state` mapping.
contract TxSimulator {

    /// Shared state — conflicts emerge when two txs touch the same key.
    mapping(bytes32 => uint256) public state;

    /// @param reads       hashed keys this tx reads
    /// @param writes      hashed keys this tx writes
    /// @param gasUsed     original gas consumed on mainnet
    function execute(
        bytes32[] calldata reads,
        bytes32[] calldata writes,
        uint256 gasUsed
    ) external {
        uint256 target = gasUsed / 100;
        uint256 count = 0;

        while (count < target) {
            for (uint256 i = 0; i < reads.length && count < target; i++) {
                state[reads[i]];
                count++;
            }
            for (uint256 i = 0; i < writes.length && count < target; i++) {
                state[writes[i]] = 1;
                count++;
            }
        }
    }
}
