import { describe, it, expect } from "vitest";
import { formatHash, parseHashInput, truncateMiddle, isHashAddress, isValoper, verificationLabel, toBig } from "~/lib/format";
import { coin } from "~/lib/ipc";

describe("amounts", () => {
  it("formats uhash as HASH without floats", () => {
    expect(formatHash("1000000")).toBe("1.00");
    expect(formatHash("1")).toBe("0.000001");
    expect(formatHash("1000000000000000")).toBe("1,000,000,000.00");
    expect(formatHash("60900000000000", 0, 0)).toBe("60,900,000");
    expect(toBig("1000000000000000")).toBe(1_000_000_000_000_000n);
  });
  it("parses typed HASH into uhash", () => {
    expect(parseHashInput("1")).toBe(1_000_000n);
    expect(parseHashInput("0.000001")).toBe(1n);
    expect(parseHashInput("12.5")).toBe(12_500_000n);
    expect(parseHashInput("1,000")).toBe(1_000_000_000n);
    expect(parseHashInput("1.0000001")).toBeNull();
    expect(parseHashInput("abc")).toBeNull();
    expect(parseHashInput("")).toBeNull();
  });
  it("reads coin lists, coin objects and strings", () => {
    expect(coin([{ denom: "uhash", amount: "5" }])).toBe("5");
    expect(coin([{ denom: "other", amount: "5" }])).toBe("0");
    expect(coin({ denom: "uhash", amount: "7" })).toBe("7");
    expect(coin("9")).toBe("9");
    expect(coin(undefined)).toBe("0");
  });
});

describe("addresses", () => {
  it("accepts hash1 and refuses cosmos1", () => {
    expect(isHashAddress("hash13t8v5nnghrvgcuuqcrt9k5wyhtqwq7fl3ynjpy")).toBe(true);
    expect(isHashAddress("cosmos1qypqxpq9qcrsszg2pvxq6rs0zqg3yyc5lzv7xu")).toBe(false);
    expect(isHashAddress("hash1short")).toBe(false);
    expect(isValoper("hashvaloper127zemcfnxd3jrldpjzzgcckek4dswyw0l7rfcq")).toBe(true);
  });
  it("truncates in the middle", () => {
    expect(truncateMiddle("hash13t8v5nnghrvgcuuqcrt9k5wyhtqwq7fl3ynjpy")).toBe("hash13t8v5…3ynjpy");
    expect(truncateMiddle("short")).toBe("short");
  });
});

describe("verification sentence", () => {
  it("is honest about single operators", () => {
    expect(verificationLabel({ agreed: true, single_operator: false, peers: ["a", "b"] })).toBe("verified by 2 nodes");
    expect(verificationLabel({ agreed: false, single_operator: true, peers: ["a"] })).toBe("verified by 1 node · single operator on network");
    expect(verificationLabel(null, { kind: "local_node" })).toBe("from the node on this PC");
    expect(verificationLabel(null, null)).toBe("not verified");
  });
});
