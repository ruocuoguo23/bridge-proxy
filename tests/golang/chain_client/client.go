package chain_client

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"time"
)

// BlockchainClient represents a client for blockchain RPC calls
type BlockchainClient struct {
	client      *http.Client
	rpcEndpoint string
	requestID   int
}

// NewBlockchainClient creates a new blockchain client with proxy configuration
func NewBlockchainClient(proxyAddr, rpcEndpoint string) (*BlockchainClient, error) {
	proxyURL, err := url.Parse(proxyAddr)
	if err != nil {
		return nil, fmt.Errorf("invalid proxy URL: %w", err)
	}

	client := &http.Client{
		Transport: &http.Transport{
			Proxy: http.ProxyURL(proxyURL),
		},
		Timeout: 10 * time.Second,
	}

	return &BlockchainClient{
		client:      client,
		rpcEndpoint: rpcEndpoint,
		requestID:   1,
	}, nil
}

// GetChainID retrieves the chain ID from the blockchain
func (c *BlockchainClient) GetChainID() (string, error) {
	return c.callRPC("eth_chainId", []interface{}{})
}

// GetBlockNumber retrieves the latest block number
func (c *BlockchainClient) GetBlockNumber() (string, error) {
	return c.callRPC("eth_blockNumber", []interface{}{})
}

// callRPC makes a generic RPC call to the blockchain
func (c *BlockchainClient) callRPC(method string, params []interface{}) (string, error) {
	// Create request payload
	request := map[string]interface{}{
		"jsonrpc": "2.0",
		"id":      c.requestID,
		"method":  method,
		"params":  params,
	}
	c.requestID++

	// Serialize request to JSON
	payload, err := json.Marshal(request)
	if err != nil {
		return "", fmt.Errorf("error serializing request: %w", err)
	}

	// Create HTTP request
	req, err := http.NewRequest("POST", c.rpcEndpoint, bytes.NewBuffer(payload))
	if err != nil {
		return "", fmt.Errorf("error creating request: %w", err)
	}
	req.Header.Set("Content-Type", "application/json")

	// Send request
	start := time.Now()
	resp, err := c.client.Do(req)
	duration := time.Since(start)
	if err != nil {
		return "", fmt.Errorf("error sending request: %w", err)
	}
	defer resp.Body.Close()

	// Read response
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return "", fmt.Errorf("error reading response: %w", err)
	}

	// Parse response
	var response map[string]interface{}
	if err := json.Unmarshal(body, &response); err != nil {
		return "", fmt.Errorf("error parsing response: %w", err)
	}

	// Check for errors
	if errObj, ok := response["error"].(map[string]interface{}); ok {
		return "", fmt.Errorf("RPC error: %v", errObj["message"])
	}

	// Extract result
	result, ok := response["result"].(string)
	if !ok {
		resultJSON, err := json.Marshal(response["result"])
		if err != nil {
			return "", fmt.Errorf("unexpected result type: %T", response["result"])
		}
		result = string(resultJSON)
	}

	fmt.Printf("[%s] Request completed in %v\n", method, duration)
	return result, nil
}
