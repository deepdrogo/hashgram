package indexer

import (
	"context"
	"fmt"
	"math/big"
	"strconv"
	"strings"
)

// Validators projection.
//
// `validators` mirrors x/staking's validator set as the REST gateway reports
// it, refreshed every Config.ValidatorRefreshBlocks indexed blocks. Rows for
// validators that have left the set are removed on refresh, so the table is
// always the current set rather than a history.

const stateValidatorsHeight = "validators_height" // indexed height of the last refresh

// normalizeBondStatus turns "BOND_STATUS_BONDED" into "bonded".
func normalizeBondStatus(s string) string {
	return strings.ToLower(strings.TrimPrefix(s, "BOND_STATUS_"))
}

// validatorRow is what one staking validator becomes in the projection.
type validatorRow struct {
	Operator       string
	Moniker        string
	Tokens         *big.Int
	CommissionRate string
	Status         string
	Jailed         bool
}

// validatorFromREST maps one element of `/cosmos/staking/v1beta1/validators`.
// Returns false when the element has no operator address or unparsable
// tokens.
func validatorFromREST(m map[string]any) (validatorRow, bool) {
	op := str(m, "operator_address")
	if op == "" {
		return validatorRow{}, false
	}
	tokens, ok := new(big.Int).SetString(strings.TrimSpace(str(m, "tokens")), 10)
	if !ok || tokens.Sign() < 0 {
		return validatorRow{}, false
	}
	row := validatorRow{Operator: op, Tokens: tokens, Status: normalizeBondStatus(str(m, "status"))}
	row.Jailed, _ = m["jailed"].(bool)
	if d, ok := m["description"].(map[string]any); ok {
		row.Moniker = str(d, "moniker")
	}
	if cm, ok := m["commission"].(map[string]any); ok {
		if rates, ok := cm["commission_rates"].(map[string]any); ok {
			row.CommissionRate = str(rates, "rate")
		}
	}
	return row, true
}

// syncValidators refreshes the projection and stamps every row with the
// indexed height, then drops rows that were not seen in this pass.
func (c *ChainIngester) syncValidators(ctx context.Context, height int64) error {
	const path = "/cosmos/staking/v1beta1/validators"
	// The validator set is bounded by the staking max_validators parameter,
	// so every refresh walks it from the first page. A resumed cursor would
	// see only the tail and the prune below would drop the head.
	if err := clearState(ctx, c.db, "registry_cursor:"+path); err != nil {
		return err
	}
	var rows []validatorRow
	err := c.paginate(ctx, path, "validators", func(m map[string]any) error {
		if row, ok := validatorFromREST(m); ok {
			rows = append(rows, row)
		}
		return nil
	})
	if err != nil {
		return err
	}
	tx, err := c.db.Begin(ctx)
	if err != nil {
		return err
	}
	defer tx.Rollback(ctx) //nolint:errcheck
	for _, v := range rows {
		if _, err := tx.Exec(ctx,
			`INSERT INTO validators(operator, moniker, tokens, commission_rate, status, jailed, updated_height)
			 VALUES ($1,$2,$3::numeric,$4,$5,$6,$7)
			 ON CONFLICT (operator) DO UPDATE SET moniker = EXCLUDED.moniker, tokens = EXCLUDED.tokens,
			   commission_rate = EXCLUDED.commission_rate, status = EXCLUDED.status, jailed = EXCLUDED.jailed,
			   updated_height = EXCLUDED.updated_height`,
			v.Operator, v.Moniker, v.Tokens.String(), v.CommissionRate, v.Status, v.Jailed, height); err != nil {
			return fmt.Errorf("validator %s: %w", v.Operator, err)
		}
	}
	// A validator absent from a complete listing has left the set. Only
	// prune after a full pass (paginate returned nil), which is the case here.
	if _, err := tx.Exec(ctx, `DELETE FROM validators WHERE updated_height < $1`, height); err != nil {
		return err
	}
	if err := setState(ctx, tx, stateValidatorsHeight, strconv.FormatInt(height, 10)); err != nil {
		return err
	}
	if err := tx.Commit(ctx); err != nil {
		return err
	}
	c.validatorsHeight = height
	return nil
}

// validatorsDue reports whether the projection should be refreshed at the
// given indexed height: never refreshed yet, or the configured number of
// blocks has passed since the last refresh.
func validatorsDue(last, height, every int64) bool {
	if every <= 0 {
		every = 50
	}
	return last == 0 || height-last >= every
}
