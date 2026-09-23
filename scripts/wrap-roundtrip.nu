#!/usr/bin/env nu
# Round-trip known text through a tagged PDF and check it comes back verbatim.
#
# Line wraps are where extraction goes wrong language by language: a space
# where Chinese has none, a hyphen dropped that was the author's, a word
# order flipped in right-to-left text. The source text is the oracle: each
# sample is laid out in several column widths so the wraps land somewhere
# new each time, printed to a tagged PDF with headless Chrome, read back
# with `pdfrum extract markdown`, and must appear in the output exactly,
# runs of whitespace aside.
#
# Needs Chrome or Chromium and fonts for the scripts below (Noto covers
# them). Not part of CI, which has neither; run it when touching line
# joining, lists or reading order:
#
#   nu scripts/wrap-roundtrip.nu              # every sample
#   nu scripts/wrap-roundtrip.nu --lang ja    # one language
#   nu scripts/wrap-roundtrip.nu --keep       # leave the PDFs for a look

# `kind` is the element the text is set in: `p`, or `li` for a list item,
# which comes back after its `1. ` marker. `known` is why a sample cannot
# come back, when it cannot: it is reported, and does not fail the run.
const SAMPLES = [
    [lang, kind, text, known];
    [zh-Hant, p, "證券商應以自己之計算買賣有價證券，並應依主管機關之規定辦理相關之申報事項，不得違反本法。", ""]
    [zh-Hant, li, "經主管機關核准之自己之計算買賣有價證券者。", ""]
    [zh-Hans, p, "本办法自发布之日起施行，原有规定与本办法不一致的，以本办法为准。", ""]
    [zh-Hant, p, "本系統支援 PDF 與 HTML 兩種格式，並可輸出 Markdown 檔案供後續處理。", ""]
    [ja, p, "この法律は、公布の日から起算して六月を超えない範囲内において政令で定める日から施行する。", ""]
    [ja, p, "コンピュータ・プログラムのソースコードは、テキストファイルとして保存されます。", ""]
    [ko, p, "이 법은 공포한 날부터 시행한다. 다만, 제5조의 개정규정은 공포 후 6개월이 경과한 날부터 시행한다.", ""]
    [en, p, "The parser recovers from a damaged cross-reference table and reports what it had to drop along the way.", ""]
    [en, p, "The state-of-the-art and up-to-date tools keep the hyphens of their compound words, as in COVID-19 and non-English.", ""]
    [en, p, "Soft hyphens let a typesetter break inter\u{ad}nation\u{ad}alization and extra\u{ad}ordinarily long words, and vanish.", ""]
    [de, p, "Die Bundesrepublik Deutschland ist ein demokratischer und sozialer Bundesstaat mit Verfassungsgerichtsbarkeit.", ""]
    [ru, p, "Настоящий закон вступает в силу со дня его официального опубликования в установленном порядке.", ""]
    [el, p, "Ο παρών νόμος ισχύει από τη δημοσίευσή του στην Εφημερίδα της Κυβερνήσεως, εκτός αν ορίζεται διαφορετικά.", ""]
    [vi, p, "Luật này có hiệu lực thi hành kể từ ngày được công bố trên Công báo của nước Cộng hòa.", ""]
    [th, p, "พระราชบัญญัตินี้ให้ใช้บังคับตั้งแต่วันถัดจากวันประกาศในราชกิจจานุเบกษาเป็นต้นไป", ""]
    [hi, p, "यह अधिनियम उस तारीख को प्रवृत्त होगा जो केंद्रीय सरकार राजपत्र में अधिसूचना द्वारा नियत करे।", ""]
    [ar, p, "يعمل بهذا القانون من تاريخ نشره في الجريدة الرسمية ويلغى كل حكم يخالف أحكامه.", ""]
    [he, p, "חוק זה ייכנס לתוקף ביום פרסומו ברשומות, אלא אם כן נקבע אחרת בחוק.", ""]
]

# Narrow enough that every sample wraps, and different enough that the
# wraps fall between different characters.
const WIDTHS = [7 11 15 22]

def find-chrome [] {
    for name in [google-chrome chromium chromium-browser google-chrome-stable] {
        if (which $name | is-not-empty) { return $name }
    }
    print --stderr "error: no Chrome or Chromium on PATH"
    exit 2
}

def squash [text: string] { $text | str replace --all --regex '\s+' ' ' | str trim }

# Korean is set `keep-all`, wrapping between words as Korean
# books do: Chrome's default breaks inside a word too, and a wrap that may
# or may not have been a space cannot be read back either way.
def page [samples: table, width: int] {
    let body = (
        $samples
        | each {|s|
            let dir = if $s.lang in [ar he] { 'rtl' } else { 'ltr' }
            if $s.kind == 'li' {
                $'<ol lang="($s.lang)" dir="($dir)"><li>($s.text)</li></ol>'
            } else {
                $'<p lang="($s.lang)" dir="($dir)">($s.text)</p>'
            }
        }
        | str join "\n"
    )
    $'<!doctype html><html><head><meta charset="utf-8"><title>wrap</title>
<style>body{font-family:"Noto Sans","Noto Sans CJK TC","Noto Sans Thai","Noto Sans Devanagari","Noto Sans Arabic","Noto Sans Hebrew",sans-serif;font-size:16px;width:($width)em}p,li{margin:0 0 1em}[lang=ko]{word-break:keep-all}</style>
</head><body>($body)</body></html>'
}

def main [
    --lang: string   # only the samples in this language
    --keep           # keep the HTML and PDF files, and say where
    --pdfrum: string # the binary to test; default the release build
] {
    let root = ($env.FILE_PWD | path dirname)
    let pdfrum = ($pdfrum | default ($root | path join target release pdfrum))
    if not ($pdfrum | path exists) {
        print --stderr $"error: ($pdfrum) not found; `cargo build --release -p pdfrum-cli` first"
        exit 2
    }
    let chrome = (find-chrome)
    let samples = if $lang == null { $SAMPLES } else { $SAMPLES | where lang == $lang }
    let dir = (mktemp --directory --tmpdir wrap-roundtrip.XXXXXX)

    let results = (
        $WIDTHS | each {|width|
            let html = ($dir | path join $'w($width).html')
            let pdf = ($dir | path join $'w($width).pdf')
            page $samples $width | save --force $html
            ^$chrome --headless --disable-gpu --no-pdf-header-footer $'--print-to-pdf=($pdf)' $'file://($html)' out+err> /dev/null
            let lines = (^$pdfrum extract markdown $pdf | lines | each {|l| squash $l })
            let text = ($lines | str join "\n")
            $samples | each {|s|
                # A soft hyphen is where the word may break, not a character
                # of it: it comes back as nothing, wrap or no wrap.
                let source = ($s.text | str replace --all "\u{ad}" '')
                let want = if $s.kind == 'li' { $'1. (squash $source)' } else { squash $source }
                let ok = ($text | str contains $want)
                # The line most like the sample: the one that shares its
                # opening, so a miss shows what came back instead.
                let head = ($s.text | str substring 0..2)
                let got = if $ok { '' } else {
                    $lines | where {|l| $l | str contains $head } | get 0? | default '(not found)'
                }
                {width: $width, lang: $s.lang, ok: $ok, known: $s.known, want: $want, got: $got}
            }
        }
        | flatten
    )

    let misses = ($results | where not ok and known == '')
    for m in ($results | where not ok) {
        if $m.known == '' {
            print $"(ansi red)MISS(ansi reset) ($m.lang) at ($m.width)em"
        } else {
            print $"(ansi yellow)KNOWN(ansi reset) ($m.lang) at ($m.width)em: ($m.known)"
        }
        print $"  want: ($m.want)"
        print $"  got:  ($m.got)"
    }
    let by_lang = (
        $results | group-by lang | transpose lang rows
        | each {|g| {lang: $g.lang, passed: ($g.rows | where ok | length), of: ($g.rows | length)} }
    )
    print ($by_lang | table)
    if $keep { print $"files kept in ($dir)" } else { rm --recursive $dir }
    if ($misses | is-not-empty) { exit 1 }
}
