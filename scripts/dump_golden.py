#!/usr/bin/env python3
"""Dump golden fixtures from the upstream Python `laya` package for Rust parity tests.

Covers the pure-logic surface that needs no model weights: language/script detection
(`lang.analyse`), routing (`Router.route`), and email cleaning (`clean_email_body`).

Usage:
    PYTHONPATH=/path/to/laya python3 scripts/dump_golden.py [out_dir]

`out_dir` defaults to crates/laya/tests/golden/. Requires only the pure `laya` import
(no torch): routing, language detection and email cleaning load nothing heavy.
"""
import json
import os
import sys

from laya.lang import analyse
from laya.email import clean_email_body
from laya.router import Router

OUT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
    os.path.dirname(__file__), "..", "crates", "laya", "tests", "golden"
)

LANG_INPUTS = [
    "I was charged twice, please refund",
    "The quick brown fox jumps over the lazy dog.",
    "Mein Konto wurde zweimal belastet",
    "Je voudrais annuler mon abonnement, merci beaucoup",
    "Quiero cancelar mi suscripcion y necesito ayuda",
    "Voce pode me mandar a nota fiscal?",
    "Ho bisogno di aiuto con la mia fattura, grazie",
    "Ik wil mijn account opzeggen alstublieft",
    "Vreau sa anulez abonamentul meu foarte repede",
    "Ami tomake ekta message pathacchi, kintu somossa hocche",
    "私は二重に請求されました",
    "मुझे दो बार शुल्क लिया गया",
    "저는 두 번 청구되었습니다",
    "Меня дважды списали деньги",
    "لقد تم خصم المبلغ مرتين",
    "ฉันถูกเรียกเก็บเงินสองครั้ง",
    "Δύο φορές χρεώθηκα",
    "நான் இருமுறை கட்டணம் வசூலிக்கப்பட்டேன்",
    "https://github.com/foo/bar and user@acme.com",
    "v1.2.3 U.S.A. e.g. i.e.",
    "Set alpha to 0.05 and beta to 0.1",
    "OK",
    "",
    "Dmitri Petrovich Savitsky wrote the report",
    "Bonjour, je m'appelle Jean et j'habite a Paris",
    "Necesito ayuda con mi factura por favor",
    "Deu erro 500 no endpoint de login depois do update",
    "Обвинение предъявлено вчера в суде Москвы",
    "混合 text with English brand names like Apple",
    "Türkçe metin çok güzel ve ilginç bir dil",
    # Mixed: a mostly-English state whose own line reads as a non-English language (upstream #207).
    "Je voudrais annuler mon abonnement car je ne suis pas satisfait du service.\nThe subscription management page keeps throwing an error whenever I click the cancel button and nothing happens after several tries on different browsers and devices today",
    # A pasted stack trace with code syntax must NOT count as a foreign segment: stays English.
    "Please refund the duplicate charge on my account.\nTraceback: os.path failed in round(el, 2) at non_english line 42 of the payment module during the retry",
    # One accented loanword or proper noun must not pull plain English off the English checkpoint:
    # two distinct English-only function words and at most one accented word rescue it (#337).
    "Please send Jose the invoice and the refund for his order today",
    "Please send José the invoice and the refund for his order today",
    "I would like a refund for the résumé service I paid for twice this month",
    # Rescue vetoed: a diacritic rate at/above 0.06 is a real non-English vocabulary, not a loanword.
    "We met at the café for a naïve chat about the résumé and the soirée plans",
    # Rescue denied: more than one word carries a non-English letter (Swedish two/gånger), #350.
    "Jag har blivit debiterad två gånger och vill ha pengarna tillbaka nu",
    # Swedish detection (upstream feat/lang Swedish): a long sentence, a short login request that
    # carries one English-shaped token (`in`) but a distinctive Swedish phrase, a two/three-word
    # support fragment below the four-token minimum, and a Danish-overlap line that names no
    # language yet still reads non-English (nordic_overlap) so it leaves the English checkpoint.
    "jag behöver hjälp med min faktura",
    "kan inte logga in",
    "glömt mitt lösenord",
    "min konto kommer ja",
    # All-caps acronym/address lines are not foreign prose (upstream fix/lang-allcaps-acronyms): a
    # line of nothing but acronyms and place names has no lowercase, so it skipped the acronym
    # blanking and was read as prose -- `MON DES EST LA` named French, `STORES ... EL PASO` Spanish.
    # A shouted line must now show more than acronym-shaped tokens (one word-length token plus a
    # long matched stopword, or a non-English diacritic) before a language is believed.
    "MON DES EST LA",
    "STORES LOS ANGELES LAS VEGAS EL PASO CLOSED",
    # A shouted line whose umlauts carry it stays German; the ASCII spelling has no diacritic to
    # fall through to and its only matched stopword is three letters, so it routes English (one of
    # the documented upper-cased casualties).
    "WIE SPÄT IST ES IN KÖLN",
    "WIE SPAET IST ES IN KOELN",
    # Cyrillic plus a disbelieved shouted Latin tail is not English.
    "ПРИВЕТ MON DES EST LA",
    # Mixed case: emphasis capitals on the words that name the language are NOT blanked (a run of
    # `_SHOUTED_MIN_WORD` letters is a word, not an acronym), so the language survives.
    "sag mir das HEUTIGE DATUM",
    "quiero cancelar mi PEDIDO POR FAVOR",
    # Mixed case: one English line above an all-caps acronym line now reads English overall, since
    # the acronyms no longer vote in the whole-state verdict.
    "Please refund my order\nMON DES EST LA",
]

EMAIL_INPUTS = [
    "Hi team,\n\nI was charged twice this month. Please refund.\n\nThanks,\nJane\n\nOn Mon, Jan 1, 2024 at 10:00 AM John <john@x.com> wrote:\n> Original message here\n> more quoted text",
    "Please cancel my order.\n\nBest regards,\nMaria Silva\nAcme Corp\n555-1234",
    "Preciso de ajuda com o meu pedido.\n\nAtenciosamente,\nJoao",
    "I need help.\n\nSent from my iPhone",
    "This is confidential information intended solely for the addressee. If you received this email in error, please delete it.",
    "Is this confidential? I need to know before sending.",
    "Refund please.\n\nThanks for the quick reply.",
    "Ola, quero cancelar.\n\nEm 10/09/2024, Fulano <fulano@x.com> escreveu:\n> mensagem antiga",
    "Report the bug.\n\n-----Original Message-----\nFrom: someone\nSent: yesterday",
    "Body line one.\nBody line two.\n\n--\nSignature Block\nCompany Name",
    # `From:` opening ordinary prose (no address) must not be treated as a reply header.
    "From: my side the whole integration works, but I was charged twice and need a refund please.",
    # A bare `From: Name` header followed by a `Sent:` line IS a reply header and cuts.
    "I need a refund for the duplicate charge.\n\nFrom: Maria Souza\nSent: Monday\nOld quoted reply text here",
    # French mail clients (upstream feat/email French): Gmail's `Le ... a écrit :` attribution with
    # a `Cordialement,` sign-off, Apple's `Envoyé depuis mon iPhone` footer, a confidentiality
    # disclaimer, and Outlook's spaced `De :` / `Envoyé :` reply header.
    "Bonjour,\n\nJe voudrais un remboursement pour la double facturation.\n\nCordialement,\nMarie Dupont\n\nLe lun. 1 janv. 2024 à 10:00, Jean <jean@x.com> a écrit :\n> message cité ici",
    "J'ai besoin d'aide avec ma commande.\n\nEnvoyé depuis mon iPhone",
    "Ce message est confidentiel et destiné uniquement au destinataire. Si vous avez reçu ce message par erreur, veuillez le supprimer.",
    "Je veux un remboursement.\n\nDe : Marie Dupont\nEnvoyé : lundi\nancien message cité",
    # Negative: a French closing word followed by a real request is not a sign-off and stays.
    "Merci de bien vouloir traiter ma demande de remboursement rapidement.",
]

ROUTER_INPUTS = [
    "I was charged twice",
    "Mein Konto wurde zweimal belastet",
    {"message": "Quiero cancelar mi suscripcion"},
    {"message": "私は二重に請求されました"},
    "OK",
    "",
    {"subject": "Hello", "body": "Vreau sa anulez"},
    "Меня дважды списали деньги",
    "The quick brown fox jumps over the lazy dog",
    {"ticket": "Deu erro 500 no endpoint depois do update"},
    # Mostly-English state, but the customer's own field reads as Portuguese (upstream #207):
    # routes to multilingual with the "mostly English, but a line or field reads as" reason.
    {"customer": "Eu preciso de ajuda com a minha conta pois fui cobrado duas vezes",
     "log": "The server returned an internal error and the request was retried three times before it finally failed on the second attempt with a timeout on the database connection pool that was exhausted"},
    # A German field hidden behind an English field longer than the 4000-char segment-scan budget:
    # the joined window reads English and the segment scan never reaches it, so only reading each
    # string value on its own catches it (upstream #384). Routes multilingual, no mixed_segment.
    {"a_log": "please help me with the account issue and the refund for the order " * 80,
     "z_customer": "Mein Konto wurde zweimal belastet und ich brauche dringend Hilfe"},
    # Swedish routes to the multilingual checkpoint now that `lang` names it (upstream Swedish).
    "jag behöver hjälp med min faktura",
    # An all-caps acronym/address line is not foreign prose, so it no longer routes multilingual on
    # its own (upstream fix/lang-allcaps-acronyms).
    "MON DES EST LA",
    # A shouted line carried by its umlauts still routes multilingual.
    "WIE SPÄT IST ES IN KÖLN",
]


def dump_lang():
    rows = []
    for text in LANG_INPUTS:
        det = analyse(text)
        rows.append({
            "input": text,
            "script": det["script"],
            "language": det["language"],
            "is_english": det["is_english"],
            "language_undecided": det["language_undecided"],
            "diacritic_rate": det["diacritic_rate"],
            "non_latin_fraction": det["non_latin_fraction"],
            "mixed_segment": det["mixed_segment"],
        })
    return rows


def dump_email():
    return [{"input": b, "output": clean_email_body(b)} for b in EMAIL_INPUTS]


def dump_router():
    # The Rust port keeps `default="english"`, the value upstream shipped through 0.3.29.
    # Upstream flipped the stock default to `multilingual` in 0.4.0 (feat(router)!), a breaking
    # change this port deliberately does not adopt, so pin english here to match the port's
    # behaviour and keep these fixtures stable across the upstream default flip.
    r = Router(default="english")
    rows = []
    for state in ROUTER_INPUTS:
        d = r.route(state, {})
        rows.append({"input": state, "model": d["model"], "reason": d["reason"]})
    return rows


def main():
    os.makedirs(OUT, exist_ok=True)
    for name, rows in (("lang", dump_lang()), ("email", dump_email()), ("router", dump_router())):
        path = os.path.join(OUT, name + ".json")
        with open(path, "w", encoding="utf-8") as f:
            json.dump(rows, f, ensure_ascii=False, indent=2)
        print("wrote", path, "(%d cases)" % len(rows))


if __name__ == "__main__":
    main()
