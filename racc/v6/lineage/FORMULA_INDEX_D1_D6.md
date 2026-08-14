# Formula Index through Draft 6

## Decision and call compression

- Decision-view quotient: minimum exact view classes `|O / ~P|`.
- Residual channel capacity: informative channel product must cover protected residual classes.
- Prepared call count: `N_Z(x) = D_Z(x) + 1`.
- Tool-call reduction: `1 - m/K`.
- Interface-token reduction: `1 - J_Z/J_B`.

## Prefix and rewrite economics

- Exact rewritten prefix: `lcp(A||X||B, A||S||B) = |A| + lcp(X||B, S||B)`.
- Rewritten residual: `s + b - r`.
- One-request crossover: `rho*r + u*(s+b-r) < rho*(n+b)`.
- Horizon crossover: `c_c + (c_w-c_r)*(s+b-r) < T*c_r*(n-s)`.

## Causal cache and Q99

- L2 reuse: `1 - invalid_demand_weight / total_demand_weight`.
- Q99 invalid-mass bound: `invalid_weight <= 0.01 * total_weight`.
- L3 Q99 feasibility: `sum s_i r_i <= C` and `sum w_i r_i >= 0.99 sum w_i`.
- Eviction slack: `sigma = resident_weight - 0.99*valid_weight`.
- Provider-miss interface reduction: `1 - (C_view + L_control)/B_replay`.
- Poisson historical model: `h = lambda/(lambda+mu)` and Q99 iff `lambda >= 99 mu` under its assumptions.

## Pareto and work closure

- Complete campaign work: `C_n = P_n/n + (1-nu_n)H_n + nu_n F_n`.
- Saving: `S_n = 1 - C_n/B_n`.
- Certified redundant-gap closure: `Gamma = (B-Z)/(B-L)`.
- Per-resource hit path: `Z_j(h) = A_j + h H_j + (1-h)F_j`.
- Fallback reserve: `s - u + h <= B - b` in the earlier notation.

## Capability and lifecycle

- Verified candidate-set monotonicity: quality nondecreasing and minimum safe cost nonincreasing under nested fresh sets.
- Capture/drift equilibrium: retained in Draft 1 RACC-Capital paper.
- Capability lifetime value: retained in Draft 1 RACC-Capital paper.

For exact assumptions, proofs, and symbol definitions, consult the current V6 papers and the claim-lineage source path.
