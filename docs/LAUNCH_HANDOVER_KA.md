# Hashgram Mainnet — გაშვების ინსტრუქცია (Genesis VPS)

მფლობელის მოკლე ბარათი (რა წაიღო, 1%, დესკტოპის პრომპტი):
[OWNER_LAUNCH_KA.md](OWNER_LAUNCH_KA.md).
დესკტოპის AI-პრომპტი ერთი კოპირებით: [PROMPT_DESKTOP_AI.md](PROMPT_DESKTOP_AI.md).
hashgram.io — საჯარო API: [PROMPT_EXPLORER_API.md](PROMPT_EXPLORER_API.md),
საიტი (explorer + docs, შავ-თეთრი): [PROMPT_HASHGRAM_IO.md](PROMPT_HASHGRAM_IO.md).
გაშვების სკრიპტები: `scripts/launch/`.

ეს დოკუმენტი არის ზუსტი ბრძანებების თანმიმდევრობა Hashgram Mainnet-ის
გასაშვებად ამ სერვერიდან (186.241.19.230), მეორე სერვერის დასამატებლად,
როლების გასანაწილებლად და ავარიის შემთხვევაში აღდგენისთვის. ინგლისურენოვანი
სრული ვერსია: [FOUNDER_LAUNCH_RUNBOOK.md](FOUNDER_LAUNCH_RUNBOOK.md).
აქ არაფერია ნავარაუდევი: ყველა ბრძანება ამ სერვერზე ან devnet-ზე გაშვებულია.

---

## 0. რა მდგომარეობაშია სერვერი ახლა

| რა | მდგომარეობა |
| --- | --- |
| რეპოზიტორია | `/home/hashgram`, 31 commit, სუფთა სამუშაო ხე, secret-scan სუფთა |
| ბინარები | `/usr/local/bin/{hashgramd, hashgramctl, hashgram-keygen, hashgram-test-client, hashgram-node, hashgram-client, hashgram-indexer, hashgram-safety}` — ყველა მიმდინარე კოდიდან |
| სისტემური მომხმარებლები | `hashgram-chain`, `hashgram-node`, `hashgram-index`, `hashgram-safety`, `hashgram-call` |
| დირექტორიები | `/var/lib/hashgram/{chain,node,index,safety,call,backups}` — **ცარიელი**, ქსელი არ არის შექმნილი |
| კონფიგები | `/etc/hashgram/{node.toml, indexer.toml, safety.toml, safety-rules.json, safety-hashes.txt}` — შაბლონები |
| systemd | `hashgramd`, `hashgram-node`, `hashgram-indexer`, `hashgram-safety` დაინსტალირებულია, არცერთი არ არის ჩართული |
| PostgreSQL 16 | აქტიური, `hashgram_index` ბაზა და როლი შექმნილია, პაროლი `/etc/hashgram/indexer.toml`-ში (0640) |
| coturn, Prometheus, Grafana | აქტიური; მონიტორინგი მხოლოდ localhost-ზე |
| ufw | ღიაა: 22, 80, 443, 26656/tcp, 26670/tcp+udp, 3478, 5349, 49152–65535/udp |
| დისკი | 96 GB, 68 GB თავისუფალი (`mainnet-preflight` მოითხოვს ≥ 50 GB) |

Founder-ის გასაღები **არ არსებობს** არსად — შენ ქმნი, ოფლაინ მანქანაზე.

---

## 1. Founder-ის ცივი საფულე (ოფლაინ მანქანაზე, არა სერვერზე)

```bash
# სერვერიდან გადაიტანე მხოლოდ ბინარი (USB-ით), მერე გათიშე ინტერნეტი იმ მანქანაზე
scp root@186.241.19.230:/usr/local/bin/hashgram-keygen .

./hashgram-keygen new                 # 24 სიტყვა. ქაღალდზე, ორ ეგზემპლარად, ორ ადგილას.
./hashgram-keygen derive              # ჩაწერილი სიტყვებით: მისამართი უნდა დაემთხვეს ზუსტად
```

სერვერზე მიდის **მხოლოდ** `hash1...` საჯარო მისამართი. მნემონიკა არასდროს
არ იბეჯდება ქსელში ჩართულ მანქანაზე. BIP-39 checksum სიტყვების გადანაცვლებას
ვერ იჭერს — გადამოწმება მისამართით ხდება, არა იმით, რომ „მიიღო".

200,000,000 HASH-ისთვის აპარატული საფულე ან multisig უკეთესია, ვიდრე
ქაღალდი. კოდი ამას არ ითხოვს, გონიერება ითხოვს.

---

## 2. Genesis VPS — ბრძანებები თანმიმდევრობით

ყველაფერი `root`-ით. `hashgramctl` ამ სერვერზე ავტომატურად მუშაობს
`/var/lib/hashgram/chain`-ზე და ჩაწერილ ფაილებს `hashgram-chain`
მომხმარებელს გადასცემს; `hashgramd`-ს კი `--home` ცალკე უნდა.

```bash
export HASHGRAM_HOME=/var/lib/hashgram/chain     # რომ hashgramd-საც იგივე სახლი ჰქონდეს
```

### 2.1 ნოდის ინიციალიზაცია და ვალიდატორის როლი

```bash
hashgramctl init --moniker <სახელი>
hashgramctl configure-role validator
```

### 2.2 ვალიდატორის ოპერატორის ცხელი გასაღები (ამ სერვერზე, არა Founder-ის)

```bash
hashgramd keys add operator --keyring-backend file --home /var/lib/hashgram/chain
# ჩაწერე ამ გასაღების მნემონიკაც (ეს ცხელი გასაღებია, სტეიკს მართავს)
hashgramd keys show operator -a --keyring-backend file --home /var/lib/hashgram/chain     # hash1... — ეს გჭირდება 2.3-ში
```

`--keyring-backend file` პაროლს ითხოვს და გასაღებს
`/var/lib/hashgram/chain/keyring-file/`-ში ინახავს დაშიფრულად. ყველა შემდეგ
`hashgramd tx`/`gentx` ბრძანებას იგივე `--keyring-backend file` დაუმატე.
სერვისისთვის ეს გასაღები საჭირო არ არის — მხოლოდ შენ იყენებ gentx-ისთვის და
მოგვიანებით ხმის მიცემისთვის/გადარიცხვებისთვის.

### 2.3 წინასწარი genesis — Founder-ის მისამართი და გაშვების ანგარიშები

```bash
hashgramctl init-mainnet-genesis \
  --founder-address hash1<FOUNDER_საჯარო_მისამართი> \
  --genesis-account hash1<operator_მისამართი_2.2-დან>=1000000HASH
```

- მთელი მიწოდება (1,000,000,000) genesis-ში უკვე განაწილებულია, ამიტომ
  ვალიდატორის სტეიკი **Founder-ის განბლოკილი 20,000,000-დან** გამოდის:
  Founder-ს რჩება 199,000,000 (180,000,000 vesting უცვლელია), launch-ანგარიშს
  1,000,000. ხაზინა და რეზერვი ხელუხლებელია. ჯამი ზუსტად 1,000,000,000.
- თუ სხვა ვალიდატორებიც არიან გაშვებაზე, ყოველისთვის ცალკე
  `--genesis-account`. ჯამი ≤ 20,000,000 HASH.
- დაბეჭდილი ცხრილში შეამოწმე Founder-ის მისამართი **სიმბოლო-სიმბოლო**.
  დაადასტურე `CREATE MAINNET`.
- დაბეჭდილი hash **წინასწარია** — ჯერ არ ჩაწერო როგორც ქსელის იდენტობა.

### 2.4 gentx

```bash
hashgramd genesis gentx operator 900000000000uhash \
  --chain-id hashgram-1 \
  --home /var/lib/hashgram/chain --keyring-backend file \
  --moniker <სახელი> \
  --commission-rate 0.10 --commission-max-rate 0.20 --commission-max-change-rate 0.01 \
  --ip 186.241.19.230
```

900,000 HASH სტეიკი, 100,000 რჩება საკომისიოებისთვის და ხმის მიცემისთვის.
სხვა ვალიდატორები იმავეს აკეთებენ თავიანთ სერვერზე შენი წინასწარი
`genesis.json`-ით და გამოგიგზავნიან `config/gentx/gentx-*.json`-ს — ჩააგდე
`/var/lib/hashgram/chain/config/gentx/`-ში.

### 2.5 საბოლოო genesis და hash-ის დაფიქსირება

```bash
hashgramctl finalize-genesis
```

აგროვებს gentx-ებს, ამოწმებს, უარს ამბობს ნულ ვალიდატორზე, ითვლის საბოლოო
hash-ს და წერს `/etc/hashgram/network.json`-სა და `app.toml`-ში. მეორედ არ
გაეშვება `--force`-ის გარეშე.

```text
GENESIS HASH  ________________________________________________________________
```

**ჩაწერე ახლავე, სერვერის გარეთ.** გადაამოწმე:

```bash
sha256sum /var/lib/hashgram/chain/config/genesis.json
hashgramctl network-info
```

### 2.6 შემოწმება და გაშვება

```bash
hashgramctl mainnet-preflight        # ყველა შემოწმება PASS უნდა იყოს
sudo scripts/install/monitoring.sh   # თუ ჯერ არ გაუშვია ამ ვერსიით
hashgramctl start
hashgramctl chain-status             # სიმაღლე იზრდება
hashgramctl status
```

### 2.7 გადამოწმება (Part G)

```bash
hashgramd query bank total --denom uhash --home /var/lib/hashgram/chain
#   1000000000000000 — ზუსტად

hashgramctl wallet-info hash1<FOUNDER>
#   Balance 199,000,000 HASH; Spendable 19,000,000; Vesting 180,000,000 (96 პერიოდი)
#   თუ Spendable == Balance, vesting არ შეიქმნა — გაჩერდი.

hashgram-test-client founder verify --node tcp://127.0.0.1:26657
#   100 bps, ceiling 100

curl -s localhost:1317/hashgram/feerouter/v1/totals
```

### 2.8 გამოქვეყნება

`genesis.json` გამოაქვეყნე ერთ არხში, hash — **სულ სხვა** ორ არხში (მაგ.
GitHub release-ის აღწერა + Telegram/Twitter). ფაილი საკუთარი hash-ის გვერდით
არაფერს ამტკიცებს.

---

## 3. გაშვების შემდეგ — პირველი governance წინადადება (Part H)

Genesis-ში **არცერთი storage assigner** არ არის, ამიტომ store-ნოდები
შენახული ბაიტებისთვის ვერაფერს იღებენ, სანამ governance არ დაარეგისტრირებს
პირველ assigner-ს. ეს ამ სერვერზე შენი node-ის ოპერატორის მისამართია
(იხ. §4, `configure-role store` ქმნის მას და `hashgramctl rewards` ბეჭდავს).

```bash
hashgramctl propose add-assigner hash1<node_operator> --out proposal-add-assigner.json
# კითხულობს ცოცხალ პარამეტრებს და ერთ მისამართს ამატებს. ხელით არ დაწერო ეს ფაილი:
# MsgUpdateParams მთელ პარამეტრებს ცვლის და ხელით დაწერილი ფაილი დანარჩენს ანულებს.

hashgramd tx gov submit-proposal proposal-add-assigner.json \
  --from operator --chain-id hashgram-1 --home /var/lib/hashgram/chain --keyring-backend file \
  --gas auto --gas-adjustment 1.4 --fees 5000uhash
hashgramd query gov proposals --output json --home /var/lib/hashgram/chain   # id
hashgramd tx gov vote 1 yes --from operator --chain-id hashgram-1 \
  --home /var/lib/hashgram/chain --keyring-backend file --fees 5000uhash
```

- დეპოზიტი 10,000 HASH, დაბრუნდება როცა გავა. ოპერატორის ანგარიშს 100,000
  HASH აქვს — საკმარისია. Founder-ის ცივი გასაღები აქ არ მონაწილეობს.
- ხმის მიცემის პერიოდი Mainnet-ზე **7 დღე**, კვორუმი 40%. ერთი ვალიდატორით
  შენი ხმა 100%-ია.
- გავლის შემდეგ: `hashgramd query serviceproof params --output json` →
  `assigners` სიაში მისამართი.

Devnet-ზე (2-წუთიანი პერიოდით) გატესტილია: გავიდა, assigner დაემატა,
სხვა პარამეტრები უცვლელი.

Welcome-პროგრამა (ახალი მომხმარებლების ჯილდო) გამორთულია, სანამ attestation-
სერვისი არ არსებობს და მისი გასაღები იმავე გზით არ დარეგისტრირდება.
გამოცხადებაში ისე თქვი.

---

## 4. ქსელური სერვისები ამავე სერვერზე (Part I)

პროდაქშენში ვალიდატორის სერვერზე სხვა როლი **არ უნდა** იყოს (იხ. §6). თუ
პირველ ეტაპზე მაინც ერთი სერვერი გაქვს:

```bash
hashgramctl configure-role validator,relay,store,media,bootstrap \
  --declared-storage 200000000000 \
  --reward-address hash1<ცივი_მისამართი_რომელიც_სერვერზე_არ_არის>
# ბეჭდავს ოპერატორის მისამართს (/var/lib/hashgram/node/operator.key)

# დააფინანსე ეს მისამართი ≥ 1,100 HASH-ით (1,000 bond + საკომისიოები) operator-იდან:
hashgramd tx bank send operator hash1<node_operator> 1100000000uhash \
  --chain-id hashgram-1 --home /var/lib/hashgram/chain --keyring-backend file --fees 5000uhash

# ჩართე ავტორეგისტრაცია
sed -i 's/^auto_register_provider = false/auto_register_provider = true/' /etc/hashgram/node.toml
hashgramctl restart
hashgramctl status          # P2P node: Peer id, External addrs, roles
hashgramctl rewards         # provider registered, bond, credit
hashgramctl health
```

კლიენტებისთვის გამოაქვეყნე bootstrap-მისამართი genesis hash-თან ერთად:

```text
/ip4/186.241.19.230/udp/26670/quic-v1/p2p/<Peer id hashgramctl status-დან>
```

ზარებისთვის (სურვილისამებრ): `sudo scripts/install/coturn.sh --realm <დომენი>`,
ჯგუფური ზარებისთვის `sudo scripts/install/livekit.sh --domain <დომენი>`.

---

## 5. მეორე VPS — შეერთება

```bash
# ახალ სერვერზე:
git clone <რეპო> /home/hashgram && cd /home/hashgram
sudo scripts/install/bootstrap-ubuntu.sh        # იგივე, რაც აქ გაკეთდა
export HASHGRAM_HOME=/var/lib/hashgram/chain

hashgramctl init --moniker <სახელი>
hashgramctl join-mainnet                        # genesis, hash, seeds — ბინარშია
hashgramctl network-info                        # pin უნდა იყოს e322bc23...

hashgramctl configure-role relay,store,media,bootstrap \
  --declared-storage 500000000000 --reward-address hash1<ცივი>
# დააფინანსე დაბეჭდილი ოპერატორის მისამართი, ჩართე auto_register_provider
hashgramctl mainnet-preflight
hashgramctl start
```

პირველი სერვერის node-id: `hashgramctl node-info` პირველ სერვერზე.

**არასდროს გადაიტანო** მეორე სერვერზე: `priv_validator_key.json` (ორი
ხელმომწერი ერთი გასაღებით = 5% slash, აღდგენის გარეშე), `node_key.json`,
keyring. `hashgramctl backup` ამათ არც ინახავს. გადატანა შეიძლება:
`genesis.json` (hash გადაამოწმე), `/etc/hashgram/network.json`,
`app.toml`/`config.toml` moniker-ის შეცვლით.

მეორე ვალიდატორი გინდა? **ახალი** consensus-გასაღები, ცალკე სტეიკი,
`MsgCreateValidator` (ან gentx გაშვებამდე). ჭარბი საიმედოობა გინდა?
sentry-არქიტექტურა, არა გასაღების კოპირება.

---

## 6. როლების განაწილება

| სერვერი | როლები | რატომ |
| --- | --- | --- |
| 1 (ეს) | `validator` მხოლოდ | signing key-ს გვერდით არაფერი მძიმე/სახიფათო |
| 2 | `relay,store,media,bootstrap` | საჯარო P2P, დისკი, გამტარობა; შემოსავალი |
| 3 | `indexer,safety` | უცხო კონტენტს ამუშავებს; systemd-ით ჩაკეტილი chain/node დირექტორიებისგან |
| 4 (სურვ.) | `call` + coturn/LiveKit | 49152–65535/udp ღიაა |

დაუშვებელი კომბინაციები: `validator`+`safety`, `validator`+`call`,
`validator`+ნებისმიერი CPU-მძიმე. დეტალები: [NODE_ROLES.md](NODE_ROLES.md).

---

## 7. ავარიული აღდგენა (DR) მოკლედ

**სერვერი კომპრომეტირებულია:**
```bash
hashgramctl stop                    # 1. შეწყვიტე ხელმოწერა ახლავე
# 2. consensus-გასაღები დაკარგულად მიიჩნიე; სხვაგან არ აღადგინო
# 3. მანქანა თავიდან ააგე, არ „გაასუფთავო"
# 4. ახალი consensus-გასაღები ახალ მანქანაზე; ძველი — არასდროს
# 5. SSH და ყველა გასაღები, რაც ამ მანქანას მიუწვდებოდა — შეცვალე
hashgramctl network-info            # 7. სწორ ქსელზე ხარ?
```
რას იღებს თავდამსხმელი: ჯაჭვის საჯარო ბაზა, P2P გასაღები, validator key
(თუ remote signer არ არის). რას **არ** იღებს: Founder-ის გასაღებს (აქ არასდროს
ყოფილა), მომხმარებლების გასაღებებს, შეტყობინებების ტექსტს (MLS ciphertext;
ტესტი ამოწმებს, რომ store-ბაზაში plaintext არ დევს), moneta-ს ბეჭდვის ან
გაყინვის შესაძლებლობას (ასეთი ტრანზაქცია არ არსებობს).

**სარეზერვო კოპია:**
```bash
hashgramctl backup                  # კონფიგები, network pin, node/index/proof state; გასაღებების გარეშე
# priv_validator_key.json და node_key.json ცალკე, ოფლაინ, 0600
```
სრულად: [DISASTER_RECOVERY.md](DISASTER_RECOVERY.md).

**Genesis-სერვერის დაკარგვა ქსელს არ კლავს**, თუ ვალიდატორები რამდენიმეა:
`phase2.sh` ტესტში genesis-ვალიდატორი და bootstrap-ნოდი ერთად მოკვდა, ჯაჭვი
გაგრძელდა, კლიენტებმა მესიჯები მესამე ნოდით გააგზავნეს. **ერთი ვალიდატორით
ეს არ მუშაობს** — ერთი სერვერის დაკარგვა = ქსელის დაკარგვა. ეს კოდის
პრობლემა არ არის, დეპლოიმენტისაა.

---

## 8. რა არის აშენებული და რა — არა (პატიოსნად)

**აშენებული და ტესტირებული:** ჯაჭვი (8 მოდული, genesis, `hashgramctl`),
P2P ნოდი (QUIC/TCP, Kademlia, Gossipsub, ხელის ჩამორთმევა genesis hash-ით),
MLS E2EE მესიჯინგი store-and-forward-ით, ხელმოწერილი სოციალური ივენთები (12
ტიპი, reels/stories ჩათვლით), content-addressed მედია რეპლიკაციით,
useful-service ჯილდოები (რეგისტრაცია, challenge-ები Merkle-მტკიცებით,
receipt-ები), Rust SDK + `hashgram-client`, Go indexer (PostgreSQL) და safety
engine, TURN კრედენშალები და ზარის სიგნალიზაცია, fuzz/audit/CI, 58-შემოწმებიანი
ქსელური acceptance (`scripts/testnet/phase2.sh`), 351 Go + 188 Rust ტესტი.

**არ არის:**
- Windows/iOS/Android აპლიკაციები — მხოლოდ სპეცები (`docs/PROMPT_*.md`), SDK-ს
  UniFFI/C-ABI ბაინდინგები აპების პირველი სამუშაოა.
- WebRTC მედია-სტეკი SDK-ში (TURN და სიგნალიზაცია არის; მედია აპის საქმეა).
- SFU ჯგუფური ზარების E2EE SFU-ს ოპერატორისგან; call-receipt-ები კლიენტიდან.
- Push-შეტყობინებები (კლიენტი პოლავს ან შეერთებული რჩება).
- Safety OCR/ვიდეო-კადრები (hook-ები არის, ეტაპები — hash, ტექსტი, HTTP მოდელი).
- Welcome attestation სერვისი; ტოკენ-bridge.
- **დამოუკიდებელი აუდიტი და მეორე იმპლემენტაცია.** ტესტები მტკიცებულება არ არის.
- რეალური NAT-ების და გეოგრაფიულად დაშორებული ოპერატორების ქცევა — ყველაფერი
  ერთ ჰოსტზეა გატესტილი.

**ცენტრალიზაციის წერტილები დღეს:** ერთი სერვერი, Founder-ის 20% ხმა
governance-ში, assigner-ების სეტი (შენ ასახელებ), welcome attestor
(სანდოა), bootstrap-სია ბინარში. სია: [DECENTRALIZATION.md](DECENTRALIZATION.md).

---

## 9. ყოველდღიური ბრძანებები

```bash
hashgramctl status | health | chain-status | peers | storage | rewards | logs -f
hashgramctl validator                # ხელმოწერის სტატუსი, missed blocks
hashgramctl backup
scripts/dev/ci.sh                    # სრული CI ლოკალურად
scripts/testnet/phase2.sh            # მთელი ქსელი ერთ მანქანაზე, 58 შემოწმება (~10 წთ, PostgreSQL სჭირდება)
```
