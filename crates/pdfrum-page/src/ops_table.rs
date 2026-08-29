// The operator table. Included by `ops.rs`; not a module of its own so the
// macro, the helper impls and the rows read as one file while keeping the
// 73-row table separately greppable.
//
// Columns: spelling => variant(operand type : ring index, …) [guard].
// Ring indices count back from the newest operand, so `1 2 m` reads x at 1
// and y at 0. `Point` and `Affine` read several slots starting at the index
// given. A trailing integer is the `param_count` an operator demands; without
// one, absent operands read as zero/empty/null.

/// The `SC`/`sc` operand set: at most four numbers, oldest first.
///
/// PDFium caps these two at four components while `SCN`/`scn` take everything
/// in the ring — a six-component `DeviceN` colour set with `sc` silently
/// loses two values and is then rejected wholesale by the component-count
/// check in [`crate::color::ColorValue::set_components`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Components(pub SmallVec<[f32; 4]>);

impl FromOperands for Components {
    fn extract(ring: &OperandRing, _index: usize) -> Self {
        Self(ring.numbers(ring.len().min(4)))
    }
}

/// The `SCN`/`scn` operand set: every number in the ring, oldest first, plus
/// the trailing pattern name when the newest operand is one.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PatternComponents {
    /// The numeric components, in source order.
    pub values: SmallVec<[f32; 4]>,
    /// The pattern named by the last operand, when there is one.
    pub pattern: Option<Name>,
}

impl FromOperands for PatternComponents {
    fn extract(ring: &OperandRing, _index: usize) -> Self {
        // `GetColors` takes all operands; `GetNamedColors` drops the trailing
        // name. A `scn` with *no* operands at all has no last operand, and
        // PDFium's `GetObject(0)` then yields null, which is not a name — so
        // it takes the numeric branch with an empty set.
        if ring.is_name(0) {
            Self {
                values: ring.named_numbers(),
                pattern: Some(Name::new(ring.string(0))),
            }
        } else {
            Self {
                values: ring.numbers(ring.len()),
                pattern: None,
            }
        }
    }
}

/// A `d` operand pair: the dash array and its phase.
///
/// Every array element is read as a float, so a non-numeric element becomes
/// `0.0`; a non-array first operand leaves the array empty *and* marks the
/// row invalid, which makes the operator a no-op.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DashPattern {
    /// The on/off lengths, verbatim — normalization happens in
    /// [`crate::state::StrokeParams`].
    pub array: SmallVec<[f32; 4]>,
    /// The distance into the pattern at which to start.
    pub phase: f32,
    /// Whether operand 1 really was an array. PDFium's handler returns
    /// without touching the state otherwise.
    pub valid: bool,
}

impl FromOperands for DashPattern {
    fn extract(ring: &OperandRing, _index: usize) -> Self {
        let phase = ring.number(0);
        match ring.object(1) {
            Object::Array(a) => Self {
                array: a.iter().map(|o| o.number().unwrap_or(0.0)).collect(),
                phase,
                valid: true,
            },
            _ => Self {
                array: SmallVec::new(),
                phase,
                valid: false,
            },
        }
    }
}

/// A `TJ` operand: the array's elements, with non-strings that are not
/// numbers dropped.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TextArray {
    /// The elements in source order.
    pub items: Box<[TextItem]>,
    /// Whether the operand was an array at all; `TJ` is a no-op otherwise.
    pub valid: bool,
}

impl FromOperands for TextArray {
    fn extract(ring: &OperandRing, _index: usize) -> Self {
        match ring.object(0) {
            Object::Array(a) => {
                let items = a
                    .iter()
                    .filter_map(|o| match o {
                        Object::Str(s) => Some(TextItem::Show(s.bytes.clone())),
                        Object::Int(_) | Object::Real(_) => {
                            o.number().map(TextItem::Adjust)
                        }
                        _ => None,
                    })
                    .collect();
                Self { items, valid: true }
            }
            _ => Self {
                items: Box::default(),
                valid: false,
            },
        }
    }
}

/// A `BDC` property operand: a name, an inline dictionary, or neither.
///
/// PDFium pushes no mark at all when the operand is null or is anything but a
/// name or a dictionary, so "neither" has to be representable.
#[derive(Debug, Clone, PartialEq)]
pub struct MarkProps(pub Option<MarkProperties>);

impl FromOperands for MarkProps {
    fn extract(ring: &OperandRing, _index: usize) -> Self {
        Self(match ring.object(0) {
            Object::Name(n) => Some(MarkProperties::Named(n)),
            Object::Dict(d) => Some(MarkProperties::Inline(Box::new(d))),
            _ => None,
        })
    }
}

ops! {
    // ---- Graphics state (11) ----

    /// `q` — push a copy of the graphics state.
    b"q" => SaveState();
    /// `Q` — pop the graphics state; a no-op on an empty stack.
    b"Q" => RestoreState();
    /// `cm` — pre-concatenate a matrix onto the CTM.
    b"cm" => Concat(Affine: 0);
    /// `w` — set the line width. Negative and zero widths are stored as-is.
    b"w" => SetLineWidth(f32: 0);
    /// `J` — set the line cap.
    b"J" => SetLineCap(LineCap: 0);
    /// `j` — set the line join.
    b"j" => SetLineJoin(LineJoin: 0);
    /// `M` — set the miter limit.
    b"M" => SetMiterLimit(f32: 0);
    /// `d` — set the dash pattern and phase.
    b"d" => SetDash(DashPattern: 0);
    /// `ri` — set the rendering intent. PDFium discards it outright; only
    /// the `/RI` `ExtGState` key is stored.
    b"ri" => SetRenderIntent(Name: 0);
    /// `i` — set the flatness tolerance. Stored, never used.
    b"i" => SetFlatness(f32: 0);
    /// `gs` — apply a named `/ExtGState` dictionary.
    b"gs" => SetExtGState(Name: 0);

    // ---- Path construction (7) ----

    /// `m` — begin a new subpath at a point.
    b"m" => MoveTo(Point: 0) 2;
    /// `l` — append a straight segment.
    b"l" => LineTo(Point: 0) 2;
    /// `c` — append a cubic Bézier with both control points given.
    b"c" => CurveTo(Point: 4, Point: 2, Point: 0);
    /// `v` — append a cubic Bézier whose first control point is the current
    /// point.
    b"v" => CurveToV(Point: 2, Point: 0);
    /// `y` — append a cubic Bézier whose second control point is its
    /// endpoint.
    b"y" => CurveToY(Point: 2, Point: 0);
    /// `h` — close the current subpath.
    b"h" => ClosePath();
    /// `re` — append a complete rectangle as a new subpath: x, y, width,
    /// height, oldest first.
    b"re" => Rectangle(f32: 3, f32: 2, f32: 1, f32: 0);

    // ---- Path painting (10) ----

    /// `S` — stroke the path.
    b"S" => Stroke();
    /// `s` — close and stroke.
    b"s" => CloseStroke();
    /// `f` — fill with the nonzero winding rule.
    b"f" => Fill();
    /// `F` — the obsolete spelling of `f`, handled identically.
    b"F" => FillObsolete();
    /// `f*` — fill with the even-odd rule.
    b"f*" => FillEvenOdd();
    /// `B` — fill then stroke, nonzero winding.
    b"B" => FillStroke();
    /// `B*` — fill then stroke, even-odd.
    b"B*" => FillStrokeEvenOdd();
    /// `b` — close, fill and stroke, nonzero winding.
    b"b" => CloseFillStroke();
    /// `b*` — close, fill and stroke, even-odd. Unlike `b`, this appends the
    /// closing segment unconditionally.
    b"b*" => CloseFillStrokeEvenOdd();
    /// `n` — end the path without painting; only a pending clip survives.
    b"n" => EndPath();

    // ---- Clipping (2) ----

    /// `W` — intersect the clip with the current path, nonzero winding.
    b"W" => Clip();
    /// `W*` — intersect the clip with the current path, even-odd.
    b"W*" => ClipEvenOdd();

    // ---- Text objects and positioning (9) ----

    /// `BT` — begin a text object, resetting both text matrices.
    b"BT" => BeginText();
    /// `ET` — end a text object, flushing any pending text clip.
    b"ET" => EndText();
    /// `Td` — move to the start of the next line, offset from the current
    /// line start.
    b"Td" => TextMove(f32: 1, f32: 0);
    /// `TD` — `Td` plus setting the leading to the negated y offset.
    b"TD" => TextMoveSetLeading(f32: 1, f32: 0);
    /// `Tm` — set the text matrix and the line matrix.
    b"Tm" => SetTextMatrix(Affine: 0);
    /// `T*` — move to the start of the next line.
    b"T*" => TextNextLine();
    /// `TL` — set the leading.
    b"TL" => SetLeading(f32: 0);
    /// `Ts` — set the text rise.
    b"Ts" => SetTextRise(f32: 0);
    /// `Tz` — set the horizontal scale, stored as the percentage over 100.
    b"Tz" => SetHorzScale(f32: 0) 1;

    // ---- Text state (4) ----

    /// `Tc` — set the character spacing.
    b"Tc" => SetCharSpace(f32: 0);
    /// `Tw` — set the word spacing.
    b"Tw" => SetWordSpace(f32: 0);
    /// `Tf` — set the font and size. The size is always taken; the font only
    /// when the name resolves.
    b"Tf" => SetFont(Name: 1, f32: 0);
    /// `Tr` — set the text rendering mode. Values outside 0..=7 leave the
    /// mode unchanged, so the operand arrives raw.
    b"Tr" => SetTextRenderMode(i64: 0);

    // ---- Text showing (4) ----

    /// `Tj` — show a string.
    b"Tj" => ShowText(PdfString: 0);
    /// `'` — move to the next line and show a string.
    b"'" => NextLineShowText(PdfString: 0);
    /// `"` — set word and character spacing, then `'`. The string is the
    /// newest operand.
    b"\"" => SetSpacingShowText(f32: 2, f32: 1, PdfString: 0);
    /// `TJ` — show strings with individual position adjustments.
    b"TJ" => ShowTextAdjusted(TextArray: 0);

    // ---- Type 3 glyph metrics (2) ----

    /// `d0` — declare a coloured Type 3 glyph's advance.
    b"d0" => Type3Width(f32: 1, f32: 0);
    /// `d1` — declare an uncoloured Type 3 glyph's advance and bounding box,
    /// oldest first.
    b"d1" => Type3WidthBBox(f32: 5, f32: 4, f32: 3, f32: 2, f32: 1, f32: 0);

    // ---- Colour (12) ----

    /// `CS` — set the stroking colorspace, resetting the colour to its
    /// default.
    b"CS" => SetStrokeColorSpace(Name: 0);
    /// `cs` — set the non-stroking colorspace.
    b"cs" => SetFillColorSpace(Name: 0);
    /// `SC` — set stroking colour components, at most four.
    b"SC" => SetStrokeColor(Components: 0);
    /// `sc` — set non-stroking colour components, at most four.
    b"sc" => SetFillColor(Components: 0);
    /// `SCN` — set stroking colour components, optionally naming a pattern.
    b"SCN" => SetStrokeColorN(PatternComponents: 0);
    /// `scn` — set non-stroking colour components, optionally naming a
    /// pattern.
    b"scn" => SetFillColorN(PatternComponents: 0);
    /// `G` — set the stroking colour to a `DeviceGray` level.
    b"G" => SetStrokeGray(f32: 0);
    /// `g` — set the non-stroking colour to a `DeviceGray` level.
    b"g" => SetFillGray(f32: 0);
    /// `RG` — set the stroking colour to a `DeviceRGB` triple.
    b"RG" => SetStrokeRgb(f32: 2, f32: 1, f32: 0) 3;
    /// `rg` — set the non-stroking colour to a `DeviceRGB` triple.
    b"rg" => SetFillRgb(f32: 2, f32: 1, f32: 0) 3;
    /// `K` — set the stroking colour to a `DeviceCMYK` quadruple.
    b"K" => SetStrokeCmyk(f32: 3, f32: 2, f32: 1, f32: 0) 4;
    /// `k` — set the non-stroking colour to a `DeviceCMYK` quadruple.
    b"k" => SetFillCmyk(f32: 3, f32: 2, f32: 1, f32: 0) 4;

    // ---- XObjects and shading (2) ----

    /// `Do` — paint a named form or image `XObject`.
    b"Do" => DoXObject(Name: 0);
    /// `sh` — paint a named shading across the clip region.
    b"sh" => ShadeFill(Name: 0);

    // ---- Inline images (3) ----

    /// `BI` — begin an inline image. Reaching dispatch means the tokenizer
    /// abandoned the image, so the operator itself does nothing.
    b"BI" => BeginInlineImage();
    /// `ID` — a stray `ID` outside a `BI`. A no-op.
    b"ID" => InlineImageData();
    /// `EI` — a stray `EI` outside a `BI`. A no-op.
    b"EI" => EndInlineImage();

    // ---- Marked content (5) ----

    /// `BMC` — begin a marked-content sequence with no properties.
    b"BMC" => BeginMarkedContent(Name: 0);
    /// `BDC` — begin a marked-content sequence with a property list.
    b"BDC" => BeginMarkedContentDict(Name: 1, MarkProps: 0);
    /// `EMC` — end a marked-content sequence; never pops past the sentinel.
    b"EMC" => EndMarkedContent();
    /// `MP` — a marked-content point with no properties. A no-op.
    b"MP" => MarkPoint(Name: 0);
    /// `DP` — a marked-content point with a property list. A no-op.
    b"DP" => MarkPointDict(Name: 1, MarkProps: 0);

    // ---- Compatibility (2) ----

    /// `BX` — begin a compatibility section. PDFium never registers this, so
    /// it falls through as an unknown keyword and is ignored; we name it so
    /// the enum is complete.
    b"BX" => BeginCompat();
    /// `EX` — end a compatibility section. Ignored, like `BX`.
    b"EX" => EndCompat();
}
