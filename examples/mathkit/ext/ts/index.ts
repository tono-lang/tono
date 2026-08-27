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

  // A static constructor on the class itself, the shape many libraries use
  // for parsing (`Type.parse(text)`): reached through the type, not through
  // a free function and not through an instance.
  static parse<T>(expr: string): FormulaCalculator<T> {
    return new FormulaCalculator<T>(expr);
  }

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

export class AnswerCalculator implements Calculator<number> {
  compute(): number {
    return 42;
  }
}

// Takes the class itself and constructs it (`new () => T`), the shape a
// library uses when the caller chooses the implementation and the library
// owns its construction: reached with the class as a value, never with an
// instance the caller built.
export function instantiate<T>(clazz: new () => Calculator<T>): Calculator<T> {
  return new clazz();
}

// A session with a remote calculation service. Its name is the one every
// generated SDK's entry takes as well, on purpose: a spelling naming it is
// the library's, and the generated code must import it from here.
export class Client {
  constructor(private readonly addr: string) {}

  ping(): string {
    return `pong from ${this.addr}`;
  }
}

export function open(addr: string): Client {
  return new Client(addr);
}

// Keeps one value of the caller's own type: the library is generic over a
// type it never sees, the way a settings or cache library is.
export class Memo<T> {
  constructor(private readonly value: T) {}

  recall(): T {
    return this.value;
  }
}

export function remember<T>(value: T): Memo<T> {
  return new Memo<T>(value);
}

// A provider generic over a bound: the library takes only an object shape
// (`T extends object`), the way a settings or cache library does when it
// spreads or freezes what it holds. The bound is the point of this shape: a
// handle whose method answers `unknown` cannot instantiate T, whatever the
// call site is annotated with.
export interface Bounded<T extends object> {
  read(): Promise<T>;
}

export class PinnedBounded<T extends object> implements Bounded<T> {
  constructor(private readonly value: T) {}

  async read(): Promise<T> {
    return this.value;
  }
}

// Holds nothing and fails every read: what a fallback falls back from.
export class AbsentBounded<T extends object> implements Bounded<T> {
  async read(): Promise<T> {
    throw new Error("mathkit: nothing to read");
  }
}

// Composes two providers the caller already built into a third: reads the
// first and answers the second when the first fails.
export function boundedFallback<T extends object>(
  a: Bounded<T>,
  b: Bounded<T>,
): Bounded<T> {
  return {
    async read(): Promise<T> {
      try {
        return await a.read();
      } catch {
        return b.read();
      }
    },
  };
}

// Where a field's value comes from, for a library that fills the caller's
// own type: one entry per field the caller wants filled.
export interface Mapping {
  from: string;
}

// What instantiateInto answers: the filled value, and the table it was
// filled from, handed back as the library holds it.
export interface Filled<T extends object> extends Bounded<T> {
  mappings(): Map<string, Mapping>;
}

// A library that constructs the caller's own type: it takes the class
// itself (`new () => T`) and a table saying where each field's value comes
// from, builds the instance and fills it. The caller owns T; the library
// never sees a value of it before it makes one, so nothing the caller
// built can stand in for the class. The table is a Map, the shape a
// library keeps a keyed collection in, not the plain object a generated
// map type is.
export function instantiateInto<T extends object>(
  name: string,
  clazz: new () => T,
  mappings: Map<string, Mapping>,
): Filled<T> {
  return {
    async read(): Promise<T> {
      if (mappings.size === 0) {
        throw new Error(`mathkit: ${name} has no mappings`);
      }
      const value = new clazz();
      for (const [field, mapping] of mappings) {
        Object.assign(value, { [field]: mapping.from });
      }
      return value;
    },
    mappings(): Map<string, Mapping> {
      return mappings;
    },
  };
}
