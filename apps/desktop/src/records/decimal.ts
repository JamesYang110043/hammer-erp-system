/**
 * 十進位運算（`bigint`，不經過浮點數，D9）：金額、數量以字串輸入與輸出，
 * 四捨五入一律 `MidpointAwayFromZero`（與伺服器的 `rust_decimal` 相同）。
 */

export interface Dec {
  /** 去掉小數點的整數值。 */
  units: bigint;
  /** 小數位數。 */
  scale: number;
}

const DEC_RE = /^([-+]?)(\d*)(?:\.(\d*))?$/;

export function parse(raw: unknown): Dec | undefined {
  if (typeof raw !== 'string') {
    return undefined;
  }
  const m = DEC_RE.exec(raw.trim());
  if (!m) {
    return undefined;
  }
  const [, sign = '', int = '', frac = ''] = m;
  if (int === '' && frac === '') {
    return undefined;
  }
  const units = BigInt(`${int || '0'}${frac}`);
  return { units: sign === '-' ? -units : units, scale: frac.length };
}

const pow10 = (n: number) => 10n ** BigInt(n);

export function rescale(d: Dec, scale: number): Dec {
  return scale >= d.scale ? { units: d.units * pow10(scale - d.scale), scale } : round(d, scale);
}

/** 四捨五入到 `dp` 位（`MidpointAwayFromZero`）。 */
export function round(d: Dec, dp: number): Dec {
  if (d.scale <= dp) {
    return rescale(d, dp);
  }
  const divisor = pow10(d.scale - dp);
  const negative = d.units < 0n;
  const abs = negative ? -d.units : d.units;
  let q = abs / divisor;
  if ((abs % divisor) * 2n >= divisor) {
    q += 1n;
  }
  return { units: negative ? -q : q, scale: dp };
}

export function mul(a: Dec, b: Dec): Dec {
  return { units: a.units * b.units, scale: a.scale + b.scale };
}

export function add(a: Dec, b: Dec): Dec {
  const scale = Math.max(a.scale, b.scale);
  return { units: rescale(a, scale).units + rescale(b, scale).units, scale };
}

export function sub(a: Dec, b: Dec): Dec {
  return add(a, { units: -b.units, scale: b.scale });
}

/** 補齊（或四捨五入）到 `scale` 位小數的字串，與資料庫讀出的格式相同。 */
export function format(d: Dec, scale: number): string {
  const v = rescale(d, scale);
  const negative = v.units < 0n;
  const digits = (negative ? -v.units : v.units).toString().padStart(scale + 1, '0');
  const int = digits.slice(0, digits.length - scale);
  const frac = digits.slice(digits.length - scale);
  return `${negative ? '-' : ''}${int}${scale > 0 ? `.${frac}` : ''}`;
}

/**
 * 除法，四捨五入到 `dp` 位（`MidpointAwayFromZero`）。除數為 0 時回傳 `undefined`。
 */
export function div(a: Dec, b: Dec, dp: number): Dec | undefined {
  if (b.units === 0n) {
    return undefined;
  }
  // a / b = (a.units / 10^a.scale) / (b.units / 10^b.scale)；放大 10^dp 後做整數除法
  let num = a.units * pow10(b.scale + dp);
  let den = b.units * pow10(a.scale);
  if (den < 0n) {
    num = -num;
    den = -den;
  }
  const negative = num < 0n;
  const abs = negative ? -num : num;
  let q = abs / den;
  if ((abs % den) * 2n >= den) {
    q += 1n;
  }
  return { units: negative ? -q : q, scale: dp };
}

/** 零。 */
export const ZERO: Dec = { units: 0n, scale: 0 };
