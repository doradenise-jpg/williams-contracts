# Implementation Index: TotalLocked TTL Maintenance Policy

This document serves as a roadmap to all implementation artifacts for the TotalLocked TTL maintenance policy.

## Quick Navigation

### For Different Audiences

**🏃 I'm in a hurry → 2 minute read**
- Start: `CHANGES_README.md`
- Then: `SOLUTION.md` (executive summary section)

**📊 I need to understand the solution → 10 minute read**
- Start: `SOLUTION.md`
- Then: `VERIFICATION_GUIDE.md` (acceptance criteria section)

**🔧 I'm implementing/reviewing code → 15 minute read**
- Start: `contracts/will/src/storage.rs` (new functions)
- Then: `contracts/will/src/lib.rs` (search "refresh_total_locked_ttl_for_balances")
- Then: `contracts/will/src/total_locked_ttl_maintenance_test.rs` (test cases)

**🏗️ I need architectural context → 20 minute read**
- Start: `docs/adr/0003-total-locked-ttl-maintenance.md`
- Then: `docs/adr/0002-total-locked-by-token-scalability.md` (for context)

**🚀 I'm deploying/operating this → 15 minute read**
- Start: `docs/TOTAL_LOCKED_TTL_MAINTENANCE.md`
- Then: Review monitoring/operational sections

## Document Map

### Root Level Documentation

| Document | Purpose | Audience | Read Time |
|----------|---------|----------|-----------|
| `CHANGES_README.md` | Quick overview of changes | Everyone | 2 min |
| `SOLUTION.md` | Detailed solution explanation | Architects, reviewers | 10 min |
| `IMPLEMENTATION_SUMMARY.md` | Change summary and checklist | Project managers, reviewers | 5 min |
| `VERIFICATION_GUIDE.md` | How to verify implementation | QA, reviewers | 10 min |
| `INDEX.md` | This document | Everyone | 5 min |

### Architecture Documentation

| Document | Purpose | Audience | Read Time |
|----------|---------|----------|-----------|
| `docs/adr/0003-total-locked-ttl-maintenance.md` | Full architectural decision record | Architects, maintainers | 20 min |
| `docs/adr/0002-total-locked-by-token-scalability.md` | Context: discrete storage model | Architects, context | 15 min |
| `docs/TOTAL_LOCKED_TTL_MAINTENANCE.md` | Operational guide | Operators, maintainers | 15 min |

### Code

| File | Purpose | Type | Lines |
|------|---------|------|-------|
| `contracts/will/src/storage.rs` | Implementation (modified) | Rust | 60 LOC |
| `contracts/will/src/lib.rs` | Integration (modified) | Rust | 15 LOC |
| `contracts/will/src/total_locked_ttl_maintenance_test.rs` | Tests (new) | Rust | 350 LOC |

## Reading Paths by Role

### Product Manager / Project Lead
1. `CHANGES_README.md` (2 min)
2. `SOLUTION.md` → Executive Summary + Verification sections (5 min)
3. `IMPLEMENTATION_SUMMARY.md` (3 min)
**Total: 10 minutes**

### Architect / Senior Developer
1. `SOLUTION.md` (full) (10 min)
2. `docs/adr/0003-total-locked-ttl-maintenance.md` (20 min)
3. Code review: `storage.rs` (5 min) + `lib.rs` (5 min)
**Total: 40 minutes**

### Developer (Code Review)
1. `contracts/will/src/storage.rs` - new functions (5 min)
2. `contracts/will/src/lib.rs` - integration points (5 min)
3. `contracts/will/src/total_locked_ttl_maintenance_test.rs` - tests (10 min)
4. `VERIFICATION_GUIDE.md` - code verification section (5 min)
**Total: 25 minutes**

### QA / Tester
1. `VERIFICATION_GUIDE.md` (full) (10 min)
2. `contracts/will/src/total_locked_ttl_maintenance_test.rs` (10 min)
3. Test execution commands section (5 min)
**Total: 25 minutes**

### DevOps / Operations
1. `docs/TOTAL_LOCKED_TTL_MAINTENANCE.md` (15 min)
2. `SOLUTION.md` → Cost Analysis section (5 min)
3. Monitoring recommendations section (5 min)
**Total: 25 minutes**

## Implementation Checklist

- [x] Code implementation complete
  - [x] `refresh_total_locked_ttl()` in storage.rs
  - [x] `refresh_total_locked_ttl_for_balances()` in storage.rs
  - [x] Integration in `check_in()`
  - [x] Integration in `emergency_checkin()`
  - [x] Integration in `batch_check_in()`

- [x] Testing complete
  - [x] Test module created
  - [x] 5 test cases implemented
  - [x] Module declared in lib.rs

- [x] Documentation complete
  - [x] ADR 0003 written
  - [x] Operational guide written
  - [x] Solution document written
  - [x] Verification guide written
  - [x] Change summary written

- [x] Acceptance criteria verified
  - [x] Criterion 1: Documented maintenance path
  - [x] Criterion 2: Active aggregates stay alive
  - [x] Criterion 3: Inactive aggregates not refreshed

## Key Files Reference

### Primary Implementation
```
contracts/will/src/storage.rs
  └── Lines ~207-283: New functions
  
contracts/will/src/lib.rs
  └── Lines ~1415-1427: check_in integration
  └── Lines ~1757-1772: emergency_checkin integration
  └── Lines ~1585-1598: batch_check_in integration
```

### Tests
```
contracts/will/src/total_locked_ttl_maintenance_test.rs
  └── 5 test functions covering all scenarios
```

### Architecture
```
docs/adr/0003-total-locked-ttl-maintenance.md
  └── Full decision record (10 KB)
```

### Operations
```
docs/TOTAL_LOCKED_TTL_MAINTENANCE.md
  └── Operational guide (8 KB)
```

## Verification Commands

### Verify Code Changes
```bash
grep "pub fn refresh_total_locked_ttl" contracts/will/src/storage.rs
grep "refresh_total_locked_ttl_for_balances" contracts/will/src/lib.rs
```

### Verify Tests
```bash
test -f contracts/will/src/total_locked_ttl_maintenance_test.rs && echo "✓ Tests exist"
grep -c "^fn test_" contracts/will/src/total_locked_ttl_maintenance_test.rs
```

### Verify Documentation
```bash
test -f docs/adr/0003-total-locked-ttl-maintenance.md && echo "✓ ADR exists"
test -f docs/TOTAL_LOCKED_TTL_MAINTENANCE.md && echo "✓ Ops guide exists"
```

### Run Tests (requires Rust)
```bash
cd /workspaces/williams-contracts
cargo test total_locked_ttl_maintenance --lib
```

## Related Documents

**Previous ADRs:**
- `docs/adr/0002-total-locked-by-token-scalability.md` - Discrete per-token storage model

**Other Resources:**
- Stellar Soroban documentation on ledger TTL and archival
- Williams contract README and documentation

## Questions?

**Question** | **See**
-----------|-------
What changed? | `CHANGES_README.md`
Why? | `SOLUTION.md` → Problem Analysis
How? | `SOLUTION.md` → Solution Design
Is it correct? | `VERIFICATION_GUIDE.md`
How do I operate it? | `docs/TOTAL_LOCKED_TTL_MAINTENANCE.md`
What are the edge cases? | `docs/adr/0003-total-locked-ttl-maintenance.md` → Edge Cases
How to verify code? | `VERIFICATION_GUIDE.md` → Verify Code Changes
How to run tests? | `VERIFICATION_GUIDE.md` → Run Tests

## Version Control

- **Date Implemented**: November 24, 2024
- **Status**: Ready for Review and Deployment
- **Related Issues**: TotalLocked TTL maintenance policy
- **Related ADRs**: ADR 0003 (this implementation), ADR 0002 (context)

## Notes for Maintainers

1. If Stellar's block time changes, update `DAY_IN_LEDGERS`, `LIFETIME_THRESHOLD`, `BUMP_AMOUNT` in `storage.rs`
2. If `MAX_TOKENS` increases significantly, monitor cost scaling
3. For production, track check-in frequency and token diversity (see ops guide)
4. Maintenance is opt-in via `check_in` calls; inactive wills expire naturally

---

**Last Updated**: November 24, 2024
**Implementation Status**: ✅ Complete
**Deployment Status**: Ready for Review
