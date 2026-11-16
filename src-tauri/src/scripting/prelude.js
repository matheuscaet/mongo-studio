// The built-in engine's shell: BSON types, the `db` handle, collections and
// cursors, in the mongosh dialect where it can be. Every call to MongoDB
// goes through the native `__native(op, db, coll, args)`, which takes and
// returns canonical Extended JSON; `toEJSON` and `fromEJSON` translate.
(() => {
  const G = globalThis;
  const native = G.__native;
  const newObjectId = G.__new_object_id;
  const newUuid = G.__new_uuid;
  const logLine = G.__log;

  // --- BSON types ---

  const HEX24 = /^[0-9a-fA-F]{24}$/;
  const INT64_MIN = -(2n ** 63n);
  const INT64_MAX = 2n ** 63n - 1n;

  function ObjectId(value) {
    if (!(this instanceof ObjectId)) return new ObjectId(value);
    let hex;
    if (value === undefined || value === null) hex = newObjectId();
    else if (value instanceof ObjectId) hex = value.$oid;
    else if (typeof value === "string" && HEX24.test(value)) hex = value;
    else if (typeof value === "object" && typeof value.$oid === "string" && HEX24.test(value.$oid)) hex = value.$oid;
    else throw new TypeError("ObjectId takes 24 hex characters, got " + describe(value));
    // an own property, so `id.$oid` keeps working in older scripts
    this.$oid = hex.toLowerCase();
  }
  ObjectId.prototype.toString = function () { return this.$oid; };
  ObjectId.prototype.toHexString = ObjectId.prototype.toString;
  ObjectId.prototype.toJSON = function () { return { $oid: this.$oid }; };
  ObjectId.prototype.equals = function (other) {
    if (other === null || other === undefined) return false;
    const hex = other instanceof ObjectId ? other.$oid : typeof other === "object" ? other.$oid : other;
    return typeof hex === "string" && hex.toLowerCase() === this.$oid;
  };
  ObjectId.prototype.getTimestamp = function () {
    return new Date(parseInt(this.$oid.slice(0, 8), 16) * 1000);
  };
  ObjectId.isValid = (v) => (typeof v === "string" && HEX24.test(v)) || v instanceof ObjectId;
  ObjectId.createFromHexString = (hex) => new ObjectId(hex);

  function Long(value) {
    if (!(this instanceof Long)) return new Long(value);
    let big;
    if (value instanceof Long) big = BigInt(value.$numberLong);
    else if (typeof value === "bigint") big = value;
    else if (typeof value === "number" && Number.isInteger(value)) big = BigInt(value);
    else if (typeof value === "string" && /^\s*-?\d+\s*$/.test(value)) big = BigInt(value.trim());
    else if (value === undefined) big = 0n;
    else throw new TypeError("NumberLong takes an integer, got " + describe(value));
    if (big < INT64_MIN || big > INT64_MAX) throw new RangeError("NumberLong out of the 64-bit range: " + big);
    this.$numberLong = big.toString();
  }
  Long.prototype.valueOf = function () { return Number(this.$numberLong); };
  Long.prototype.toNumber = Long.prototype.valueOf;
  Long.prototype.toString = function () { return this.$numberLong; };
  Long.prototype.toBigInt = function () { return BigInt(this.$numberLong); };
  Long.prototype.toJSON = function () {
    const n = Number(this.$numberLong);
    return Number.isSafeInteger(n) ? n : this.$numberLong;
  };
  Long.prototype.equals = function (o) { return String(o instanceof Long ? o.$numberLong : o) === this.$numberLong; };
  Long.fromNumber = (n) => new Long(Math.trunc(n));
  Long.fromString = (s) => new Long(s);
  Long.fromBigInt = (b) => new Long(b);

  function Int32(value) {
    if (!(this instanceof Int32)) return new Int32(value);
    const n = Number(value === undefined ? 0 : value);
    if (!Number.isInteger(n) || n < -2147483648 || n > 2147483647) {
      throw new RangeError("NumberInt takes a 32-bit integer, got " + describe(value));
    }
    this.value = n;
  }
  Int32.prototype.valueOf = function () { return this.value; };
  Int32.prototype.toString = function () { return String(this.value); };
  Int32.prototype.toJSON = function () { return this.value; };

  function Double(value) {
    if (!(this instanceof Double)) return new Double(value);
    this.value = Number(value === undefined ? 0 : value);
  }
  Double.prototype.valueOf = function () { return this.value; };
  Double.prototype.toString = function () { return String(this.value); };
  Double.prototype.toJSON = function () { return this.value; };

  function Decimal128(value) {
    if (!(this instanceof Decimal128)) return new Decimal128(value);
    const text = value instanceof Decimal128 ? value.$numberDecimal : String(value === undefined ? "0" : value).trim();
    if (!/^[-+]?(\d+\.?\d*|\.\d+)([eE][-+]?\d+)?$|^[-+]?(Infinity|Inf|NaN)$/i.test(text)) {
      throw new TypeError("NumberDecimal takes a decimal number, got " + describe(value));
    }
    this.$numberDecimal = text;
  }
  Decimal128.prototype.toString = function () { return this.$numberDecimal; };
  Decimal128.prototype.valueOf = function () { return Number(this.$numberDecimal); };
  Decimal128.prototype.toJSON = function () { return { $numberDecimal: this.$numberDecimal }; };
  Decimal128.fromString = (s) => new Decimal128(s);

  function UUID(value) {
    if (!(this instanceof UUID)) return new UUID(value);
    const text = value === undefined || value === null ? newUuid() : value instanceof UUID ? value.hex : String(value);
    const hex = text.replace(/-/g, "").toLowerCase();
    if (!/^[0-9a-f]{32}$/.test(hex)) throw new TypeError("UUID takes 32 hex digits, got " + describe(value));
    this.hex = hex;
  }
  UUID.prototype.toString = function () {
    const h = this.hex;
    return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
  };
  UUID.prototype.toHexString = UUID.prototype.toString;
  UUID.prototype.toJSON = function () { return { $uuid: this.toString() }; };
  UUID.prototype.equals = function (o) { return o instanceof UUID ? o.hex === this.hex : String(o).replace(/-/g, "").toLowerCase() === this.hex; };

  const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  function hexToBase64(hex) {
    const bytes = [];
    for (let i = 0; i < hex.length; i += 2) bytes.push(parseInt(hex.slice(i, i + 2), 16));
    let out = "";
    for (let i = 0; i < bytes.length; i += 3) {
      const [a, b = 0, c = 0] = bytes.slice(i, i + 3);
      const n = (a << 16) | (b << 8) | c;
      out += B64[(n >> 18) & 63] + B64[(n >> 12) & 63];
      out += i + 1 < bytes.length ? B64[(n >> 6) & 63] : "=";
      out += i + 2 < bytes.length ? B64[n & 63] : "=";
    }
    return out;
  }
  function base64ToHex(text) {
    const clean = String(text).replace(/[^A-Za-z0-9+/]/g, "");
    let bits = 0, value = 0, hex = "";
    for (const ch of clean) {
      value = (value << 6) | B64.indexOf(ch);
      bits += 6;
      if (bits >= 8) {
        bits -= 8;
        hex += ((value >> bits) & 255).toString(16).padStart(2, "0");
      }
    }
    return hex;
  }

  function ISODate(text) {
    const date = text === undefined ? new Date() : new Date(text);
    if (Number.isNaN(date.getTime())) throw new TypeError("ISODate can't read " + describe(text));
    return date;
  }

  const BinData = (subType, base64) => ({ $binary: { base64: String(base64), subType: Number(subType).toString(16).padStart(2, "0") } });
  function Timestamp(t, i) {
    if (t && typeof t === "object") return { $timestamp: { t: t.t >>> 0, i: t.i >>> 0 } };
    return { $timestamp: { t: (t || 0) >>> 0, i: (i || 0) >>> 0 } };
  }
  const MinKey = () => ({ $minKey: 1 });
  const MaxKey = () => ({ $maxKey: 1 });

  function describe(value) {
    try {
      return typeof value === "string" ? JSON.stringify(value) : String(value);
    } catch (_) {
      return typeof value;
    }
  }

  // --- Extended JSON ---

  /** A JS value as canonical-enough Extended JSON for the native side. */
  function toEJSON(v, depth = 0) {
    if (depth > 200) throw new RangeError("the value is nested too deeply (or refers to itself)");
    if (v === null || v === undefined) return null;
    switch (typeof v) {
      case "number":
        if (Number.isFinite(v)) return v;
        return { $numberDouble: Number.isNaN(v) ? "NaN" : v > 0 ? "Infinity" : "-Infinity" };
      case "bigint":
        return { $numberLong: new Long(v).$numberLong };
      case "string":
      case "boolean":
        return v;
      case "function":
      case "symbol":
        return null;
    }
    if (v instanceof Date) {
      const t = v.getTime();
      if (Number.isNaN(t)) throw new TypeError("Invalid Date can't be stored");
      return { $date: { $numberLong: String(t) } };
    }
    if (v instanceof ObjectId) return { $oid: v.$oid };
    if (v instanceof Long) return { $numberLong: v.$numberLong };
    if (v instanceof Int32) return { $numberInt: String(v.value) };
    if (v instanceof Double) return Number.isFinite(v.value) ? { $numberDouble: String(v.value) } : toEJSON(v.value);
    if (v instanceof Decimal128) return { $numberDecimal: v.$numberDecimal };
    if (v instanceof UUID) return { $binary: { base64: hexToBase64(v.hex), subType: "04" } };
    if (v instanceof RegExp) {
      const options = v.flags.split("").filter((f) => "imsx".includes(f)).sort().join("");
      return { $regularExpression: { pattern: v.source, options } };
    }
    if (v instanceof Cursor) throw new TypeError("a cursor can't be used as a value: call toArray() on it first");
    if (typeof v.then === "function") throw new TypeError("a promise was used as a value: add await before the call");
    if (Array.isArray(v)) return v.map((x) => toEJSON(x, depth + 1));
    if (v instanceof Map) {
      const out = {};
      for (const [k, x] of v) out[String(k)] = toEJSON(x, depth + 1);
      return out;
    }
    if (v instanceof Error) return { error: String(v) };
    const out = {};
    for (const k of Object.keys(v)) out[k] = toEJSON(v[k], depth + 1);
    return out;
  }

  /** Canonical Extended JSON from the native side as JS values. */
  function fromEJSON(v) {
    if (v === null || typeof v !== "object") return v;
    if (Array.isArray(v)) {
      for (let i = 0; i < v.length; i++) v[i] = fromEJSON(v[i]);
      return v;
    }
    const keys = Object.keys(v);
    if (keys.length === 1) {
      const x = v[keys[0]];
      switch (keys[0]) {
        case "$oid": return new ObjectId(x);
        case "$date": return new Date(typeof x === "object" && x !== null ? Number(x.$numberLong) : x);
        case "$numberInt":
        case "$numberDouble": return Number(x);
        case "$numberLong": return new Long(x);
        case "$numberDecimal": return new Decimal128(x);
        case "$binary":
          if (x && (x.subType === "04" || x.subType === "4")) return new UUID(base64ToHex(x.base64));
          return v;
      }
    }
    for (const k of keys) v[k] = fromEJSON(v[k]);
    return v;
  }

  async function call(op, dbName, coll, ...args) {
    // trailing undefined arguments are dropped, so "no options" stays absent
    while (args.length && args[args.length - 1] === undefined) args.pop();
    return fromEJSON(await native(op, dbName, coll, toEJSON(args)));
  }

  // --- printing ---

  function show(value) {
    if (typeof value === "string") return value;
    if (value instanceof Error) return String(value);
    try {
      return G.__format(toEJSON(value));
    } catch (_) {
      return String(value);
    }
  }
  const log = (...args) => logLine(args.map(show).join(" "));
  G.console = { log, info: log, warn: log, error: log, debug: log };
  G.print = log;
  G.printjson = (value) => logLine(G.__format(toEJSON(value), true));

  // --- cursors ---

  const FIND_OPTIONS = new Set(["projection", "sort", "limit", "skip", "hint", "maxTimeMS", "allowDiskUse", "collation", "batchSize", "comment", "min", "max", "returnKey", "showRecordId", "noCursorTimeout", "let"]);
  /** How many documents a bare `await cursor` returns at most. */
  const BARE_LIMIT = 1000;

  function Cursor(kind, dbName, coll, spec, options) {
    this._kind = kind;
    this._db = dbName;
    this._coll = coll;
    this._spec = spec;
    this._options = Object.assign({}, options || {});
    this._id = null;
    this._buffer = [];
    this._done = false;
    this._started = false;
    this._mappers = [];
  }
  const setter = (key) => function (value) {
    if (this._started) throw new Error(`${key}() must come before the cursor is read`);
    this._options[key] = value;
    return this;
  };
  for (const key of ["sort", "limit", "skip", "hint", "maxTimeMS", "allowDiskUse", "collation", "batchSize", "comment", "min", "max", "returnKey", "showRecordId", "noCursorTimeout"]) {
    Cursor.prototype[key] = setter(key);
  }
  Cursor.prototype.allowDiskUse = function (value) { return setter("allowDiskUse").call(this, value === undefined ? true : value); };
  Cursor.prototype.noCursorTimeout = function () { return this; };
  Cursor.prototype.project = setter("projection");
  Cursor.prototype.projection = setter("projection");
  Cursor.prototype.readPref = function () { return this; };
  Cursor.prototype.map = function (fn) { this._mappers.push(fn); return this; };
  Cursor.prototype._apply = function (doc) {
    for (const fn of this._mappers) doc = fn(doc);
    return doc;
  };
  Cursor.prototype._fill = async function () {
    if (this._done) return;
    if (!this._started) {
      this._started = true;
      const page = await call(this._kind, this._db, this._coll, this._spec, this._options);
      this._id = page.id;
      this._buffer.push(...page.docs);
      this._done = page.done;
    } else {
      const page = await call("more", this._db, this._coll, this._id);
      this._buffer.push(...page.docs);
      this._done = page.done;
    }
  };
  Cursor.prototype.hasNext = async function () {
    while (!this._buffer.length && !this._done) await this._fill();
    return this._buffer.length > 0;
  };
  Cursor.prototype.next = async function () {
    if (!(await this.hasNext())) return null;
    return this._apply(this._buffer.shift());
  };
  Cursor.prototype.tryNext = Cursor.prototype.next;
  Cursor.prototype.toArray = async function () {
    const out = [];
    while (await this.hasNext()) {
      for (const doc of this._buffer.splice(0)) out.push(this._apply(doc));
    }
    return out;
  };
  Cursor.prototype.forEach = async function (fn) {
    while (await this.hasNext()) {
      for (const doc of this._buffer.splice(0)) {
        if ((await fn(this._apply(doc))) === false) {
          await this.close();
          return;
        }
      }
    }
  };
  Cursor.prototype.itcount = async function () {
    let n = 0;
    while (await this.hasNext()) n += this._buffer.splice(0).length;
    return n;
  };
  Cursor.prototype.close = async function () {
    this._buffer = [];
    if (this._id !== null && !this._done) await call("close", this._db, this._coll, this._id);
    this._done = true;
  };
  Cursor.prototype.isExhausted = function () { return this._done && !this._buffer.length; };
  Cursor.prototype.count = function () {
    if (this._kind !== "find") throw new Error("count() works on find cursors only");
    const { limit, skip, hint, maxTimeMS, collation } = this._options;
    return call("countDocuments", this._db, this._coll, this._spec, clean({ limit, skip, hint, maxTimeMS, collation }));
  };
  Cursor.prototype.size = Cursor.prototype.count;
  Cursor.prototype.explain = function (verbosity) {
    return call("explain", this._db, this._coll, this._kind, this._spec, this._options, verbosity || "queryPlanner");
  };
  Cursor.prototype[Symbol.asyncIterator] = function () {
    return { next: async () => ((await this.hasNext()) ? { value: await this.next(), done: false } : { value: undefined, done: true }) };
  };
  /**
   * `await cursor` without toArray(): the documents, up to 1000 for a find
   * without a limit, as the engine always did - toArray() reads them all.
   */
  Cursor.prototype.then = function (resolve, reject) {
    const bare = async () => {
      if (this._kind === "find" && this._options.limit === undefined && !this._started) {
        this._options.limit = BARE_LIMIT + 1;
        const docs = await this.toArray();
        if (docs.length > BARE_LIMIT) {
          log(`(showing the first ${BARE_LIMIT} documents: call toArray() for all of them, or set a limit)`);
          docs.length = BARE_LIMIT;
        }
        return docs;
      }
      return this.toArray();
    };
    return bare().then(resolve, reject);
  };

  function clean(options) {
    const out = {};
    for (const k of Object.keys(options)) if (options[k] !== undefined) out[k] = options[k];
    return out;
  }

  /** mongosh's find(filter, projection, options), or the driver's find(filter, options). */
  function findOptions(second, third) {
    if (second === undefined || second === null) return clean(Object.assign({}, third || {}));
    const keys = Object.keys(second);
    const isOptions = keys.length > 0 && keys.every((k) => FIND_OPTIONS.has(k));
    if (isOptions) return clean(Object.assign({}, second, third || {}));
    return clean(Object.assign({ projection: second }, third || {}));
  }

  // --- collections and databases ---

  function Collection(dbName, name) {
    this._db = dbName;
    this._name = name;
  }
  const C = Collection.prototype;
  C.getName = function () { return this._name; };
  C.getFullName = function () { return this._db + "." + this._name; };
  C.toString = C.getFullName;
  C.getDB = function () { return makeDb(this._db); };
  C.find = function (filter, projection, options) {
    return new Cursor("find", this._db, this._name, filter || {}, findOptions(projection, options));
  };
  C.findOne = function (filter, projection, options) {
    return call("findOne", this._db, this._name, filter || {}, findOptions(projection, options));
  };
  C.aggregate = function (pipeline, options) {
    if (!Array.isArray(pipeline)) {
      // aggregate(stage1, stage2, ...) as in old shells
      pipeline = Array.from(arguments).filter((s) => s && typeof s === "object");
      options = undefined;
    }
    return new Cursor("aggregate", this._db, this._name, pipeline, options);
  };
  const plain = (op) => function (...args) { return call(op, this._db, this._name, ...args); };
  for (const op of ["countDocuments", "estimatedDocumentCount", "distinct", "insertOne", "insertMany", "updateOne", "updateMany", "replaceOne", "deleteOne", "deleteMany", "findOneAndUpdate", "findOneAndReplace", "findOneAndDelete", "bulkWrite", "createIndex", "createIndexes", "dropIndex", "dropIndexes", "drop", "stats"]) {
    C[op] = plain(op);
  }
  C.getIndexes = plain("getIndexes");
  C.getIndices = C.getIndexes;
  C.count = function (filter, options) { return call("countDocuments", this._db, this._name, filter || {}, options); };
  C.insert = function (docs, options) {
    return Array.isArray(docs) ? this.insertMany(docs, options) : this.insertOne(docs, options);
  };
  C.remove = function (filter, justOne) {
    const one = justOne === true || (justOne && justOne.justOne);
    return one ? this.deleteOne(filter) : this.deleteMany(filter);
  };
  C.update = function (filter, update, options) {
    const o = Object.assign({}, options || {});
    const multi = o.multi;
    delete o.multi;
    return multi ? this.updateMany(filter, update, o) : this.updateOne(filter, update, o);
  };

  function proxied(target, child) {
    return new Proxy(target, {
      get(t, prop, receiver) {
        if (typeof prop === "symbol" || prop in t) return Reflect.get(t, prop, receiver);
        // `then` absent, so awaiting a handle isn't mistaken for a promise
        if (prop === "then" || prop.startsWith("_")) return undefined;
        return child(prop);
      },
    });
  }

  function makeCollection(dbName, name) {
    // db.system.profile: an unknown property names a sub-collection
    return proxied(new Collection(dbName, name), (prop) => makeCollection(dbName, name + "." + prop));
  }

  function checkDbName(name) {
    if (typeof name !== "string" || name === "" || /[\/\\. "$\0]/.test(name)) {
      throw new TypeError("Invalid database name: " + JSON.stringify(name));
    }
    return name;
  }

  function makeDb(dbName) {
    const d = {
      getName: () => dbName,
      toString: () => dbName,
      getSiblingDB: (name) => makeDb(checkDbName(name)),
      getCollection: (name) => {
        if (typeof name !== "string" || name === "") throw new TypeError("getCollection takes a collection name");
        return makeCollection(dbName, name);
      },
      getCollectionNames: () => call("getCollectionNames", dbName, ""),
      getCollectionInfos: (filter, options) => call("getCollectionInfos", dbName, "", filter || {}, options),
      runCommand: (command) => call("runCommand", dbName, "", typeof command === "string" ? { [command]: 1 } : command),
      adminCommand: (command) => call("runCommand", "admin", "", typeof command === "string" ? { [command]: 1 } : command),
      createCollection: (name, options) => call("createCollection", dbName, name, options),
      stats: () => call("runCommand", dbName, "", { dbStats: 1 }),
      aggregate: (pipeline, options) => new Cursor("aggregate", dbName, "", pipeline, options),
    };
    d.collection = d.getCollection;
    d.getMongo = () => ({ getDB: (name) => makeDb(checkDbName(name)), getDBNames: () => call("getDatabaseNames", "admin", "") });
    return proxied(d, (name) => makeCollection(dbName, name));
  }

  async function sleep(ms) {
    await native("sleep", "", "", [Number(ms) || 0]);
  }

  /** The script's completion value, ready for the native side. */
  async function finish(value) {
    if (value instanceof Cursor) value = await value;
    else if (value && typeof value.then === "function") value = await value;
    return toEJSON(value);
  }

  Object.assign(G, {
    ObjectId, ObjectID: ObjectId, Long, NumberLong: Long, Int32, NumberInt: Int32, Double,
    Decimal128, NumberDecimal: Decimal128, UUID, ISODate, BinData, Timestamp, MinKey, MaxKey,
    sleep, __makeDb: makeDb, __dbName: checkDbName, __finish: finish,
  });
  G.__use = function (name) {
    G.__currentDb = checkDbName(name);
    log("switched to db " + name);
    return makeDb(name);
  };
  G.db = makeDb(G.__currentDb);
})();
