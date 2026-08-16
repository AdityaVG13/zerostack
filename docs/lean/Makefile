.PHONY: build trust axiom-report verify

build:
	lake build

trust:
	python3 scripts/check_trust.py

axiom-report: build
	lake env lean ZeroRacc/AxiomReport.lean

verify: trust axiom-report
