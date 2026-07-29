# x402-list Base service submission

Date: 2026-07-29

Status: accepted by the directory form and pending manual review; no payment
was requested or made.

## Submission scope

This was a **service** submission, not a facilitator resubmission. The
submitted public service is:

- name: **Base Agent Evidence & Route API**;
- base URL and website: <https://merchant-base.mikedotexe.com/>;
- category: **Blockchain**; and
- description: paid, finality-aware Base account and transaction evidence,
  bounded activity lookup, and dry Base-USDC-to-NEAR-USDC route quotes.

The directory's automatic probe found these five paid paths valid. Immediately
before submission, each listed method/path returned an unpaid canonical x402
v2 HTTP 402:

```text
POST /v1/evidence/account
POST /v1/evidence/transaction
POST /v1/activity/search
POST /v1/routes/usdc/quote
GET  /v1/entities/0x0000000000000000000000000000000000000000
```

The submission notes disclosed the Base mainnet payment policy: canonical
Circle USDC at `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913`, fixed `1000`
atomic units (`$0.001000`) to
`0x7Ff46ab88688D528bCE3e59c470240c6901cF88c`, with EIP-712 domain
`USD Coin` / `2`. They also linked the public discovery documents and the
source/deployment evidence without including the submitter email.

## Directory result

At submission time, the x402-list form displayed:

> Submission received. Probe found 5 valid endpoint(s). Your service will
> appear in the directory after manual approval.

The form did not display a submission ID. It did not present the documented
`$0.50` rejected-resubmission challenge. No payment signature, wallet funding,
or on-chain transaction was created for this submission.

## Deployed provenance

The directory probe targeted the immutable Base merchant release built from
merged `main` commit
[`000bf1f7d501a6f3e79ce320165019b4d00ae95a`](https://github.com/fastnear/x402-facilitator/commit/000bf1f7d501a6f3e79ce320165019b4d00ae95a),
installed as
`git-000bf1f7d501a6f3e79ce320165019b4d00ae95a`. Its archive SHA-256 is
`16cd02746784b2d3b9061ea3797fc52788fad21924a387e3db56df71293e1190`.
The prior [merchant provenance rollout](2026-07-29-merchant-provenance-rollout.md)
records the full unpaid deployment and monitoring checks.

## Boundaries

The service is operator-owned and its Base recipient is facilitator-controlled.
The accepted service submission is discovery progress, not evidence of an
independently operated merchant or organic settlement volume. It does not
change the independent Base-adoption gate or the 2026-08-03 earliest date for
any future facilitator resubmission recorded in the
[2026-07-28 facilitator review](2026-07-28-x402-list-review.md).
