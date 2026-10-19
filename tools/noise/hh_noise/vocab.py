"""Localized words used by the operators (labels, notes, titles, month names)."""

LANGS = ("en", "de", "fr", "es", "it", "nl", "sr")

MONTHS = {
    "en": ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"],
    "de": ["Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August", "September", "Oktober", "November", "Dezember"],
    "fr": ["janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre"],
    "es": ["enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre"],
    "it": ["gennaio", "febbraio", "marzo", "aprile", "maggio", "giugno", "luglio", "agosto", "settembre", "ottobre", "novembre", "dicembre"],
    "nl": ["januari", "februari", "maart", "april", "mei", "juni", "juli", "augustus", "september", "oktober", "november", "december"],
    "sr": ["januar", "februar", "mart", "april", "maj", "jun", "jul", "avgust", "septembar", "oktobar", "novembar", "decembar"],
}
MONTHS_ABBR = {
    "en": ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"],
    "de": ["Jan", "Feb", "Mär", "Apr", "Mai", "Jun", "Jul", "Aug", "Sep", "Okt", "Nov", "Dez"],
    "fr": ["janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.", "déc."],
    "es": ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic"],
    "it": ["gen", "feb", "mar", "apr", "mag", "giu", "lug", "ago", "set", "ott", "nov", "dic"],
    "nl": ["jan", "feb", "mrt", "apr", "mei", "jun", "jul", "aug", "sep", "okt", "nov", "dec"],
    "sr": ["jan", "feb", "mar", "apr", "maj", "jun", "jul", "avg", "sep", "okt", "nov", "dec"],
}

TOTAL = {
    "en": ["Total", "TOTAL", "Grand Total", "Total:", "Sum"],
    "de": ["Summe", "Gesamt", "Insgesamt", "Gesamtsumme"],
    "fr": ["Total", "Total général", "TOTAL", "Somme"],
    "es": ["Total", "Total general", "TOTAL", "Suma"],
    "it": ["Totale", "Totale generale", "TOTALE"],
    "nl": ["Totaal", "Eindtotaal", "TOTAAL"],
    "sr": ["Ukupno", "UKUPNO", "Svega", "Sveukupno"],
}
SUBTOTAL = {  # {g} = group value
    "en": ["Total {g}", "{g} Total", "Subtotal", "Subtotal {g}", "{g} - Total"],
    "de": ["Summe {g}", "{g} gesamt", "Zwischensumme"],
    "fr": ["Total {g}", "Sous-total", "Sous-total {g}"],
    "es": ["Total {g}", "Subtotal", "Subtotal {g}"],
    "it": ["Totale {g}", "Subtotale", "Subtotale {g}"],
    "nl": ["Totaal {g}", "Subtotaal", "Subtotaal {g}"],
    "sr": ["Ukupno {g}", "Svega {g}", "Međuzbir"],
}
ROW_TOTAL_HDR = {"en": "Total", "de": "Gesamt", "fr": "Total", "es": "Total", "it": "Totale", "nl": "Totaal", "sr": "Ukupno"}
SOURCE = {"en": "Source:", "de": "Quelle:", "fr": "Source :", "es": "Fuente:", "it": "Fonte:", "nl": "Bron:", "sr": "Izvor:"}
NOTE = {"en": ["Note:", "Notes:", "NB:", "Comment:"], "de": ["Anmerkung:", "Hinweis:"], "fr": ["Remarque :", "Note :"],
        "es": ["Nota:", "Notas:"], "it": ["Nota:", "Note:"], "nl": ["Opmerking:", "Noot:"], "sr": ["Napomena:", "Napomene:"]}
NOTE_TEXT = {
    "en": ["Figures are preliminary and subject to revision.", "Totals may not add up due to rounding.",
           "Data as reported by the respective departments.", "Excludes discontinued operations.",
           "Last updated by finance team.", "Values in italics are estimates.", "Please do not distribute."],
    "de": ["Vorläufige Ergebnisse.", "Rundungsdifferenzen möglich.", "Ohne Sondereffekte.", "Stand der Daten siehe oben."],
    "fr": ["Données provisoires.", "Les totaux peuvent différer en raison des arrondis.", "Hors activités abandonnées."],
    "es": ["Datos provisionales.", "Los totales pueden no coincidir por redondeo.", "Cifras sujetas a revisión."],
    "it": ["Dati provvisori.", "I totali possono non coincidere per arrotondamenti.", "Valori stimati in corsivo."],
    "nl": ["Voorlopige cijfers.", "Totalen kunnen door afronding afwijken.", "Exclusief beëindigde activiteiten."],
    "sr": ["Podaci su preliminarni.", "Zbirovi se mogu razlikovati zbog zaokruživanja.", "Procena za poslednji period."],
}
SOURCES = ["company records", "internal database", "annual report", "national statistical office", "finance department",
           "Eurostat", "World Bank", "OECD", "RZS", "ERP export"]
SCALE = {  # (thousands, millions)
    "en": ["(in thousands)", "in thousands of USD", "(000s)", "USD '000", "($ in millions)"],
    "de": ["(in Tausend EUR)", "in Tsd. EUR", "(Mio. EUR)"],
    "fr": ["(en milliers d'euros)", "en milliers", "(en millions d'euros)"],
    "es": ["(en miles de euros)", "en miles", "(millones de euros)"],
    "it": ["(in migliaia di euro)", "in migliaia", "(milioni di euro)"],
    "nl": ["(x 1.000 euro)", "in duizenden euro", "(x € 1 mln)"],
    "sr": ["(u hiljadama dinara)", "u 000 RSD", "(u milionima dinara)"],
}
TITLE_PREFIX = {
    "en": ["", "Report: ", "Summary of ", "Overview - ", "Monthly ", "Analysis of "],
    "de": ["", "Bericht: ", "Übersicht ", "Auswertung "],
    "fr": ["", "Rapport : ", "Synthèse ", "Tableau "],
    "es": ["", "Informe: ", "Resumen de ", "Cuadro "],
    "it": ["", "Rapporto: ", "Riepilogo ", "Tabella "],
    "nl": ["", "Rapport: ", "Overzicht ", "Tabel "],
    "sr": ["", "Izveštaj: ", "Pregled ", "Tabela "],
}
AS_OF = {"en": "As of {d}", "de": "Stand: {d}", "fr": "Au {d}", "es": "A {d}", "it": "Al {d}", "nl": "Per {d}", "sr": "Stanje na dan {d}"}
PREPARED = {"en": "Prepared by: {n}", "de": "Erstellt von: {n}", "fr": "Préparé par : {n}", "es": "Elaborado por: {n}",
            "it": "Preparato da: {n}", "nl": "Opgesteld door: {n}", "sr": "Pripremio: {n}"}
PEOPLE = ["J. Smith", "M. Keller", "A. Rossi", "P. Novak", "L. Garcia", "S. Dubois", "K. Jansen", "T. Lay", "R. Buy"]
ORGS = ["Acme Corp.", "Northwind Ltd", "Enron North America", "Contoso GmbH", "Global Trading Inc.", "RZS", "Ministry of Finance"]
STRAY = ["OK", "check!", "see note", "draft", "x", "TBD", "?", "v2", "updated", "per JS", "revised", "do not use", "FYI", "ok - LG"]
GROUP_FMT = ["{g}", "{h}: {g}", "{g}:", "{G}", "– {g} –", "{h} {g}"]
TABLE_CAPTION = {"en": "Table {n}", "de": "Tabelle {n}", "fr": "Tableau {n}", "es": "Cuadro {n}", "it": "Tabella {n}",
                 "nl": "Tabel {n}", "sr": "Tabela {n}"}
PARENT_HEADERS = {
    "en": ["Measures", "Values", "Statistics", "Financials", "Details", "Key figures", "Results", "Indicators"],
    "de": ["Kennzahlen", "Werte", "Ergebnisse", "Angaben"],
    "fr": ["Indicateurs", "Valeurs", "Résultats", "Données"],
    "es": ["Indicadores", "Valores", "Resultados", "Datos"],
    "it": ["Indicatori", "Valori", "Risultati", "Dati"],
    "nl": ["Kengetallen", "Waarden", "Resultaten", "Gegevens"],
    "sr": ["Pokazatelji", "Vrednosti", "Rezultati", "Podaci"],
}
UNITS = {"money": ["$", "USD", "€", "EUR", "£", "CHF", "RSD", "din."], "count": ["pcs", "kom.", "units"],
         "float": ["kg", "t", "km", "m²", "kWh", "MWh", "l"]}
BOOL_STYLES = [("Yes", "No"), ("Y", "N"), ("YES", "NO"), ("1", "0"), ("TRUE", "FALSE"), ("✓", ""), ("x", ""), ("Da", "Ne"), ("Ja", "Nein"), ("Oui", "Non")]
