// A stand-in for a third-party numeric library with the shapes real
// TypeScript libraries have, kept deliberately (this is the FFI bench, not a
// fixture bent to fit the emitter): every constructor is a class, reached
// with `new`, not a factory function; the formula options are an optional
// object; the series constructor takes an array; the fallback constructor
// takes an array of the handles the caller already built; and compute() is
// synchronous, unlike the Go and Rust shapes of the same contract.

export interface Calculator<T> {
  compute(): T;
}

export class ConstantCalculator<T> implements Calculator<T> {
  constructor(private readonly value: T) {}

  compute(): T {
    return this.value;
  }
}

export interface FormulaOptions {
  precision?: number;
}

export class FormulaCalculator<T> implements Calculator<T> {
  constructor(
    private readonly expr: string,
    private readonly opts?: FormulaOptions,
  ) {}

  // Evaluates "<a> <op> <b>" for number results (the only instantiation the
  // bench exercises); any other T fails loudly.
  compute(): T {
    const fields = this.expr.trim().split(/\s+/);
    if (fields.length !== 3) {
      throw new TypeError(`mathkit: cannot parse ${JSON.stringify(this.expr)}`);
    }
    const a = Number(fields[0]);
    const b = Number(fields[2]);
    if (Number.isNaN(a) || Number.isNaN(b)) {
      throw new Error(`mathkit: cannot parse ${JSON.stringify(this.expr)}`);
    }
    let out: number;
    switch (fields[1]) {
      case "+":
        out = a + b;
        break;
      case "-":
        out = a - b;
        break;
      case "*":
        out = a * b;
        break;
      case "/":
        out = a / b;
        break;
      default:
        throw new Error(
          `mathkit: unknown operator ${JSON.stringify(fields[1])}`,
        );
    }
    const digits = this.opts?.precision;
    if (digits !== undefined && digits > 0) {
      const scale = 10 ** digits;
      out = Math.round(out * scale) / scale;
    }
    return out as unknown as T;
  }
}

export class SeriesCalculator<T> implements Calculator<T> {
  constructor(private readonly values: T[]) {}

  // Answers the last value of the series.
  compute(): T {
    if (this.values.length === 0) {
      throw new Error("mathkit: empty series");
    }
    const [last] = this.values.slice(-1);
    return last;
  }
}

export class FallbackCalculator<T> implements Calculator<T> {
  constructor(
    private readonly strategy: string,
    private readonly calcs: Calculator<T>[],
  ) {
    if (strategy !== "first" && strategy !== "last") {
      throw new Error("mathkit: unknown fallback strategy");
    }
  }

  // Asks each composed calculator in turn ("first" answers the first
  // success, "last" the last one).
  compute(): T {
    let last: T | undefined;
    let lastError: unknown = new Error(
      "mathkit: no calculators to fall back to",
    );
    for (const c of this.calcs) {
      try {
        const v = c.compute();
        if (this.strategy === "first") {
          return v;
        }
        last = v;
        lastError = undefined;
      } catch (e) {
        lastError = e;
      }
    }
    if (lastError !== undefined) {
      throw lastError;
    }
    return last as T;
  }
}
