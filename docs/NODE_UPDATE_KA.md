# Genesis VPS — `hashgram-node`-ის განახლება (მესიჯები და @username-ები რომ ამუშავდეს)

> **2026-09-13 დამატება — Explore / Hashwall (აპლიკაცია 0.3.x).**
> აპლიკაციის Explore გვერდი (Walls, People, Hashtags, Posts) კვანძს
> ახალ მოთხოვნებს უგზავნის: `SocialDigest` და `EventFetch` დროის
> ფილტრებით (`before_timestamp`, `hashtag`, `channel`, `latest`). სერვერზე
> ახლა გაშვებული ბინარი მათ არ იცნობს — `SocialDigest`-ზე პასუხობს
> `invalid: empty request`, `EventFetch`-ზე კი **ცარიელ სიას** აბრუნებს.
> აპლიკაცია 0.3.1-დან ამას თვითონ ამოიცნობს და ჩვენებას ლოკალური ქეშიდან
> აწყობს შენიშვნით „From this device…“, მაგრამ **ქსელის** კედლები,
> ტოპ-ავტორები და ჰეშთეგები მხოლოდ კვანძის განახლების შემდეგ გამოჩნდება.
> ნაბიჯები იგივეა, რაც ქვემოთ, §2 — `git pull`, `cargo build --release
> --locked -p hashgram-node`, `install`, `systemctl restart hashgram-node`.
> პირველი გაშვებაზე კვანძი სოციალური ივენთების ინდექსს ხელახლა აწყობს
> (`INDEX_VERSION 2`; ლოგში `social: derived indexes rebuilt`), რამდენიმე წამი.
>
> ამავე განახლებაში კვანძი ლოგში `info` დონეზე წერს ყოველი დაკარგული
> კავშირის მიზეზს (`connection closed peer=… transport=quic|tcp cause=…`).
> თუ კლიენტები ხშირად „ოფლაინზე გადიან“ და ჟურნალში ჩანს, რომ ერთი და
> იგივე კლიენტი ხელახლა ვერ უკავშირდება, სანამ ძველი კავშირი არ მოკვდება,
> `/etc/hashgram/node.toml`-ში `max_connections_per_peer = 4` დაუწერეთ
> (default 2) და გადატვირთეთ.

ეს დოკუმენტი აღწერს, რატომ არ მუშაობდა Windows აპლიკაციაში მიმოწერა და
@username-ით ძებნა, და ზუსტად რა უნდა გაკეთდეს სერვერზე (186.241.19.230),
რომ ამუშავდეს. Windows აპლიკაციის მხარის შესწორებები უკვე ამ რეპოზიტორიაშია
(იხ. ბოლო სექცია); სერვერის ნაბიჯების გარეშე ისინი საკმარისი არ არის.

---

## 1. რა დადგინდა (2026-09-12, პირდაპირ Mainnet კვანძთან შემოწმებით)

Windows-იდან `hashgram-client`-ით კვანძს (`12D3KooWF53M…`, 186.241.19.230:26670)
დაუკავშირდით და გავარკვიეთ:

| ფაქტი | შედეგი |
| --- | --- |
| handshake | გადის; კვანძი აცხადებს როლებს `bootstrap, media, relay, store` |
| `KeyPackagePublish` / `MailboxFetch` (store სერვისი) | მუშაობს |
| `AnnounceQuery` | მუშაობს |
| **`ChainQuery`** (ჯაჭვის წაკითხვა P2P-ით: ბალანსი, `identity/devices`, `username/lookup`) | **კვანძი პასუხობს `invalid: empty request`** |

`empty request` ნიშნავს, რომ სერვერზე გაშვებული `hashgram-node` ბინარი
**არ იცნობს `ChainQuery` შეტყობინებას** — ის აშენებულია
2026-09-10-ის კომიტამდე `c9470f4 node: chain relay over /hashgram/rpc/1`,
რომელმაც chain relay დაამატა (Mainnet-ის გაშვების იმავე დღეს, გაშვების
შემდეგ). Windows აპლიკაციას ჯაჭვთან სხვა გზა არ აქვს (1317 პორტი
სერვერზე მხოლოდ localhost-ზეა, HTTPS endpoint არ არის კონფიგურირებული),
ამიტომ:

- `@username` → მისამართის ძებნა ვერ სრულდება (ჯაჭვის წაკითხვაა);
- მესიჯის გაგზავნა ვერ სრულდება — მიმღების მოწყობილობის გასაღებები
  ჯაჭვიდან (`hashgram/identity/v1/devices/{addr}`) იკითხება;
- ბალანსი/identity/რეგისტრაცია — იგივე.

ეს **ერთი მიზეზია სამივე სიმპტომისთვის** ყველა კომპიუტერზე.

დამატებით, კვანძის მხარეს ორი პარამეტრი უშლიდა ხელს „ხან უკავშირდება, ხან არა“:

- `max_connections_per_subnet = 4` — ერთი /24 ქსელიდან (ერთი ოფისი,
  ერთი სახლი, მობილური ოპერატორის NAT) მაქსიმუმ 4 შემომავალი კავშირი;
  ძველი კლიენტი თითო კომპიუტერიდან 2–3 კავშირს ხსნიდა, ე.ი. მე-2/მე-3
  კომპიუტერი უკვე უარყოფილი იყო (ჩუმად, `debug` ლოგით).
- ეს რიცხვი ახალ კოდში 32-ია და უარყოფა `info` დონეზე იწერება ჟურნალში.

## 2. რა უნდა გაკეთდეს სერვერზე (root-ით, ~15 წუთი)

```bash
# 1) კოდის განახლება
cd /home/hashgram
git fetch origin && git checkout main && git pull --ff-only
git log -1 --oneline          # უნდა იყოს ≥ ეს კომიტი (chain relay + Windows შესწორებები)

# 2) Rust სტეკის აწყობა (hashgram-node, hashgram-client)
cd node
cargo build --release --locked -p hashgram-node -p hashgram-client
cd ..

# 3) ბინარების დაყენება და სერვისის გადატვირთვა
install -m 0755 -o root -g root node/target/release/hashgram-node   /usr/local/bin/hashgram-node
install -m 0755 -o root -g root node/target/release/hashgram-client /usr/local/bin/hashgram-client
systemctl restart hashgram-node

# 4) შემოწმება — ორივე ხაზი უნდა გამოჩნდეს
journalctl -u hashgram-node --since "2 min ago" --no-pager | grep -E "chain relay enabled|store services enabled|peer verified"
curl -s 127.0.0.1:26672/v1/status | python3 -m json.tool | grep -E '"roles"|relay|store' 
```

თუ `chain relay enabled`-ის ნაცვლად ჩანს `chain relay off: no chain node
configured` — `/etc/hashgram/node.toml`-ში უნდა იყოს
`chain_api = "http://127.0.0.1:1317"` და `hashgramd`-ის REST API ჩართული
(`curl -s 127.0.0.1:1317/cosmos/base/tendermint/v1beta1/node_info`).

ალტერნატივა, თუ Go/Rust toolchain სერვერზე უკვე დგას:
`scripts/install/bootstrap-ubuntu.sh --binaries-only` და `hashgramctl restart`.

### 2.1 კავშირების ლიმიტი (`/etc/hashgram/node.toml`)

ახალ ბინარში default უკვე 32-ია. თუ გინდათ აშკარად ჩაწერა (ან უფრო მეტი):

```toml
# შემომავალი კავშირები ერთი /24 (IPv4) ან /64 (IPv6) ქსელიდან.
max_connections_per_subnet = 64
```

და `systemctl restart hashgram-node`.

### 2.2 საათი

კვანძი უარყოფს ხელმოწერილ მოთხოვნებს, რომელთა დროც ±300 წამზე მეტით
სცდება მის საათს. სერვერზე `timedatectl` უნდა აჩვენებდეს
`System clock synchronized: yes`. Windows აპლიკაცია ახლა თვითონ აჩვენებს
გაფრთხილებას, თუ მომხმარებლის საათი ქსელს 4 წუთზე მეტით სცდება.

## 3. რას შეამოწმებთ Windows-იდან განახლების შემდეგ

```powershell
# რეპოს node\target\debug-ში (cargo build -p hashgram-client)
$env:HASHGRAM_PASSPHRASE = "x"
.\hashgram-client.exe --home $env:TEMP\hg configure --network mainnet --genesis-hash e322bc2319f6e0173286fa526dab5a8ff8ad0797c7b80dd03e7c9d98621d5e4d
.\hashgram-client.exe --home $env:TEMP\hg identity create --offline
.\hashgram-client.exe --home $env:TEMP\hg wallet balance      # უნდა დაბეჭდოს ბალანსი (0 HASH) და არა "empty request"
```

Windows აპლიკაციაში: Network → „Chain sources“ → **P2P relay** მწვანე,
Messages-ის ზედა ბანერი ქრება, როცა ყველა პირობა სრულდება.

## 4. რა არის საჭირო, რომ ორმა ადამიანმა მიმოწერა შეძლოს

მიმოწერა E2EE-ია (MLS) და მოწყობილობის საჯარო გასაღები **ჯაჭვზეა**.
თითოეული მხარისთვის:

1. ანგარიშს უნდა ჰქონდეს **ცოტა HASH** (რეგისტრაციის საკომისიო
   ≈ 0.0005 HASH; აპლიკაცია 0.005 HASH-ს ითხოვს მარაგით). ახალ
   მომხმარებელს ვინმემ უნდა გაუგზავნოს — `x/welcome` ჯილდო ავტომატური
   არ არის (attestor არ არის რეგისტრირებული).
2. აპლიკაცია ამის შემდეგ **თვითონ** აგზავნის `MsgCreateIdentity`-ს
   (ან `MsgAddDevice`-ს მეორე კომპიუტერზე). Settings → Notifications →
   „Register this PC's device key on chain automatically“ (ჩართულია).
3. აპლიკაცია ონლაინ უნდა იყოს ერთხელ მაინც, რომ key package-ები store
   კვანძზე დაიდოს (ავტომატურია).

Messages გვერდის ბანერი ზუსტად ამბობს, რომელი პუნქტი აკლია.

## 5. Windows აპლიკაციის მხარის შესწორებები (უკვე კოდშია, 0.1.2)

- **TCP Windows-ზე საერთოდ არ მუშაობდა** (`WSAEADDRINUSE 10048`,
  libp2p-ის port-reuse). მხოლოდ QUIC/UDP აკავშირებდა; სადაც UDP
  დაბლოკილია ან VPN-ის MTU-ა, კავშირი არ იყო. — `hashgram-p2p/src/transport.rs`.
- ერთი კვანძისკენ 2–3 კავშირი იხსნებოდა (QUIC + TCP + peerstore); ახლა
  ერთი dial ყველა მისამართით, ვინც პირველი მოვა, ის რჩება.
- ქსელის გაშვება ჩავარდებოდა, თუ „თავისუფალი“ TCP პორტის UDP წყვილი
  დაკავებული იყო; ახლა პორტი 0 (OS ირჩევს) და ერთი ტრანსპორტის ჩავარდნა
  მეორეს არ აჩერებს; აპლიკაცია გაშვებას ავტომატურად იმეორებს.
- `@username`-ის პასუხის პარსინგი არასწორი იყო (`reverse` →
  `registrations[0].name`, `lookup` → `found`/`registration.owner`) —
  სახელი არსად ჩანდა და @-ის გარეშე ძებნა არ მუშაობდა.
- ყოველ 4 წამში ახალი last-resort key package იქმნებოდა და vault
  უსასრულოდ იზრდებოდა; ახლა ერთი, 25 დღით, განახლება 15 წუთში ერთხელ
  ან Welcome-ის შემდეგ.
- DHT provider ძებნას 5 წამიანი ლიმიტი აქვს (30 წამის ნაცვლად).
- Messages-ში „მზადყოფნის“ ბანერი + ავტომატური identity რეგისტრაცია.
- ლოკალური end-to-end ტესტი: `scripts/testnet/messaging-devnet.ps1`
  (ორი კვანძი, ორი კლიენტი, რეალური swarm, MLS Welcome, ორმხრივი
  მიმოწერა) — Windows-ზე გადის.
