package app

import (
	"errors"

	"github.com/cosmos/cosmos-sdk/client"
	sdk "github.com/cosmos/cosmos-sdk/types"
	"github.com/cosmos/cosmos-sdk/x/auth/ante"
	"github.com/cosmos/cosmos-sdk/x/auth/posthandler"
)

// HandlerOptions bundles the dependencies of the Hashgram ante handler.
type HandlerOptions struct {
	ante.HandlerOptions
}

// NewAnteHandler builds the Hashgram ante handler chain.
//
// The chain is the Cosmos SDK default minus the circuit breaker: Hashgram does
// not compile in x/circuit, so there is no chain-wide message pause to check.
// Nothing else is removed. In particular signature verification, sequence
// increment and fee deduction are all present and in the SDK's order, because
// reordering them is a well-known way to introduce replay bugs.
func NewAnteHandler(options HandlerOptions) (sdk.AnteHandler, error) {
	if options.AccountKeeper == nil {
		return nil, errors.New("account keeper is required for the ante handler")
	}
	if options.BankKeeper == nil {
		return nil, errors.New("bank keeper is required for the ante handler")
	}
	if options.SignModeHandler == nil {
		return nil, errors.New("sign mode handler is required for the ante handler")
	}

	decorators := []sdk.AnteDecorator{
		// SetUpContextDecorator must be outermost: it installs the gas meter
		// and the panic recovery that turns an out-of-gas into an error
		// instead of killing the node.
		ante.NewSetUpContextDecorator(),
		ante.NewExtensionOptionsDecorator(options.ExtensionOptionChecker),
		ante.NewValidateBasicDecorator(),
		ante.NewTxTimeoutHeightDecorator(),
		ante.NewValidateMemoDecorator(options.AccountKeeper),
		ante.NewConsumeGasForTxSizeDecorator(options.AccountKeeper),
		ante.NewDeductFeeDecorator(options.AccountKeeper, options.BankKeeper, options.FeegrantKeeper, options.TxFeeChecker),
		// SetPubKeyDecorator must precede every signature-verification step.
		ante.NewSetPubKeyDecorator(options.AccountKeeper),
		ante.NewValidateSigCountDecorator(options.AccountKeeper),
		ante.NewSigGasConsumeDecorator(options.AccountKeeper, options.SigGasConsumer),
		ante.NewSigVerificationDecorator(options.AccountKeeper, options.SignModeHandler, options.SigVerifyOptions...),
		ante.NewIncrementSequenceDecorator(options.AccountKeeper),
	}

	return sdk.ChainAnteDecorators(decorators...), nil
}

func (app *HashgramApp) setAnteHandler(txConfig client.TxConfig) {
	anteHandler, err := NewAnteHandler(HandlerOptions{
		ante.HandlerOptions{
			AccountKeeper:   app.AccountKeeper,
			BankKeeper:      app.BankKeeper,
			SignModeHandler: txConfig.SignModeHandler(),
			FeegrantKeeper:  app.FeeGrantKeeper,
			SigGasConsumer:  ante.DefaultSigVerificationGasConsumer,
			SigVerifyOptions: []ante.SigVerificationDecoratorOption{
				ante.WithUnorderedTxGasCost(ante.DefaultUnorderedTxGasCost),
				ante.WithMaxUnorderedTxTimeoutDuration(ante.DefaultMaxTimeoutDuration),
			},
		},
	})
	if err != nil {
		panic(err)
	}
	app.SetAnteHandler(anteHandler)
}

func (app *HashgramApp) setPostHandler() {
	postHandler, err := posthandler.NewPostHandler(posthandler.HandlerOptions{})
	if err != nil {
		panic(err)
	}
	app.SetPostHandler(postHandler)
}
