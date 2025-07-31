package main

import (
	"bridge-proxy-test/chain_client"
	"flag"
	"fmt"
	"time"
)

func main() {
	proxyAddr := flag.String("proxy", "http://localhost:8080", "Bridge proxy address")
	flag.Parse()

	// List of chains to test
	chains := []struct {
		name     string
		endpoint string
	}{
		{"Ethereum", "https://eth.llamarpc.com"},
		{"Binance Smart Chain", "https://binance.llamarpc.com"},
		// Add more chains as needed
	}

	// Test each chain
	for _, chain := range chains {
		fmt.Printf("\n===== Testing %s =====\n", chain.name)
		client, err := chain_client.NewBlockchainClient(*proxyAddr, chain.endpoint)
		if err != nil {
			fmt.Printf("Error creating client: %v\n", err)
			continue
		}

		// Test chain ID
		fmt.Println("Getting chain ID...")
		chainID, err := client.GetChainID()
		if err != nil {
			fmt.Printf("Chain ID error: %v\n", err)
		} else {
			fmt.Printf("Chain ID: %s\n", chainID)
		}

		// Test block number
		fmt.Println("\nGetting latest block number...")
		blockNum, err := client.GetBlockNumber()
		if err != nil {
			fmt.Printf("Block number error: %v\n", err)
		} else {
			fmt.Printf("Latest block: %s\n", blockNum)
		}

		// Small delay between chains
		time.Sleep(1 * time.Second)
	}
}
