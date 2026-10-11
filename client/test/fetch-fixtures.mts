// One-off: snapshot real devnet accounts the decoder tests read (re-run to refresh).
import { writeFileSync } from "node:fs";
import { address } from "@solana/kit";
import { pdas, NPCS_MINT } from "../src/lib/programs";
import { getAccounts } from "../src/lib/rpc";
import { simulateValidate, validateIx } from "../src/lib/gate";
import { generateKeyPairSigner } from "@solana/kit";
const H = address("BJL7L58MX82DiGxtjcq5VbzWuLLZe26U6R8PCfdJ2EMh");
const CRED = address("7H1P4RLB9VhsrSHwqtxJJsMC5wMtF9T7HUN6EUovhfxV");
const ID = address("6ooYMTCt89VLStUxPPWdTq19Ee9E9TaQCnJPxt3abRUL");
const ACC = address("FVxSDQnri8zSREZTXjdgdXc2D8KPyNSFWFsUjG3BuWaU");
const named = {
  marketConfig: await pdas.gateConfig(NPCS_MINT),
  marketPolicy: await pdas.gatePolicy(NPCS_MINT),
  holderLotsH: await pdas.lots(NPCS_MINT, H),
  holderLinkH: await pdas.holderLink(NPCS_MINT, H),
  identityH: await pdas.attestation(CRED, ID, H),
  accreditationH: await pdas.attestation(CRED, ACC, H),
};
const accts = await getAccounts(Object.values(named));
const out: Record<string, { address: string; owner: string; data: string }> = {};
Object.keys(named).forEach((k, i) => {
  const a = accts[i];
  if (!a) throw new Error(`missing ${k}`);
  out[k] = { address: a.address, owner: a.owner, data: Buffer.from(a.data).toString("base64") };
});
// A wallet with no attestations: the gate's evented rejection.
const stranger = (await generateKeyPairSigner()).address;
const payer = address("9WH1WZWBSEQRpixmREZ8RFkzT9xkG9X8NvRpU3Jo2uDZ");
const ix = await validateIx(NPCS_MINT, stranger, payer, await pdas.attestation(CRED, ID, stranger), await pdas.attestation(CRED, ACC, stranger), new Uint8Array(16));
const rejected = await simulateValidate(ix, payer);
if (rejected.reject !== "AttestationInvalid") throw new Error(`unexpected decision ${JSON.stringify(rejected)}`);
writeFileSync(new URL("./fixtures/devnet-accounts.json", import.meta.url), JSON.stringify({ cluster: "devnet", mint: NPCS_MINT, holder: H, fetchedAt: new Date().toISOString(), accounts: out, validateRejectLogs: rejected.logs }, null, 2) + "\n");
console.log(Object.keys(out));
