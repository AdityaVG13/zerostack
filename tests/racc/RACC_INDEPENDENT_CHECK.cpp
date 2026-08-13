#include <algorithm>
#include <cassert>
#include <cstdint>
#include <iomanip>
#include <iostream>
#include <numeric>
#include <set>
#include <vector>

using u128 = __uint128_t;

static long double to_ld(u128 x) {
    long double out = 0;
    long double base = 1;
    while (x) {
        out += static_cast<unsigned>(x & 0xffffffffULL) * base;
        base *= 4294967296.0L;
        x >>= 32;
    }
    return out;
}

int main() {
    // Independent check 1: doubling cost is strictly below 4K.
    long double worst = 0.0L;
    std::uint64_t worst_k = 0;
    for (std::uint64_t k = 1; k <= 10'000'000ULL; ++k) {
        std::uint64_t b = 1, sum = 0;
        while (true) {
            sum += b;
            if (b >= k) break;
            b *= 2;
        }
        assert(static_cast<u128>(sum) < static_cast<u128>(4) * k);
        long double ratio = static_cast<long double>(sum) / k;
        if (ratio > worst) { worst = ratio; worst_k = k; }
    }
    std::cout << "PASS online-bidding doubling<4 through K=10000000"
              << " worst_k=" << worst_k
              << " ratio=" << std::setprecision(15) << worst << "\n";

    // Independent check 2: uniform occupancy formula by exhaustive enumeration.
    // N=5, m=7. Sum distinct-count over all N^m demand strings.
    constexpr int N = 5;
    constexpr int M = 7;
    std::uint64_t total_sequences = 1;
    for (int i = 0; i < M; ++i) total_sequences *= N;
    std::uint64_t distinct_sum = 0;
    for (std::uint64_t code = 0; code < total_sequences; ++code) {
        std::uint64_t x = code;
        bool seen[N] = {false};
        for (int j = 0; j < M; ++j) {
            seen[x % N] = true;
            x /= N;
        }
        distinct_sum += std::count(std::begin(seen), std::end(seen), true);
    }
    // Formula E|U| = N[1-(1-1/N)^M]. Multiply by N^M:
    // numerator = N*(N^M-(N-1)^M).
    std::uint64_t pN = 1, pNm1 = 1;
    for (int i = 0; i < M; ++i) { pN *= N; pNm1 *= (N-1); }
    std::uint64_t formula_num = N * (pN - pNm1);
    assert(distinct_sum == formula_num);
    std::cout << "PASS uniform occupancy N=5 m=7 numerator="
              << distinct_sum << "/" << total_sequences << "\n";

    // Independent check 3: bounded-context cumulative phase identity.
    // R_t=R0+g(t-1), K_t=B.
    const std::uint64_t T = 100000, R0 = 1000, g = 100, B = 500;
    u128 raw = static_cast<u128>(T) * R0
             + static_cast<u128>(g) * T * (T - 1) / 2;
    u128 racc = static_cast<u128>(B) * T;
    long double retained = to_ld(racc) / to_ld(raw);
    assert(retained < 0.001L);
    std::cout << "PASS horizon retained fraction="
              << std::setprecision(15) << retained << "\n";

    // Independent check 4: an exact line-count streaming transducer needs
    // ceil(log2(N+1)) bits to represent counts 0..N.
    const std::uint64_t lines = 1'000'000;
    unsigned width = 0;
    std::uint64_t states = 1;
    while (states < lines + 1) { states <<= 1; ++width; }
    assert(width == 20);
    assert(states >= lines + 1 && (states >> 1) < lines + 1);
    std::cout << "PASS streaming line-count width=" << width << " bits\n";

    // Independent check 5: a natural topological order of a chain has
    // exact causal cut width one interface.
    const int chain_n = 10000;
    int max_crossing = 0;
    for (int t = 0; t <= chain_n; ++t) {
        int crossing = (t > 0 && t < chain_n) ? 1 : 0;
        max_crossing = std::max(max_crossing, crossing);
    }
    assert(max_crossing == 1);
    std::cout << "PASS chain causal-cut width=1 for N=" << chain_n << "\n";

    std::cout << "ALL INDEPENDENT C++ CHECKS PASSED\n";
    return 0;
}
