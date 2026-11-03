#[doc = "JavaScript prelude evaluated in the sandbox before each script."]
pub const CODEMODE_PRELUDE: &str = r#"(function (bridge, configJson) {
	"use strict";

	// Freeze the built-ins the prelude shares with the script. Without this a
	// script could patch, say, JSON.stringify or Array.prototype.toJSON and
	// silently change what the host is told.
	(function lockdown() {
		const seen = new Set([globalThis]);
		const queue = [];
		const add = (value) => {
			if (((typeof value === "object" && value !== null) || typeof value === "function") && !seen.has(value)) {
				seen.add(value);
				queue.push(value);
			}
		};
		for (const key of Reflect.ownKeys(globalThis)) {
			const descriptor = Object.getOwnPropertyDescriptor(globalThis, key);
			if (!descriptor) continue;
			add(descriptor.value);
			add(descriptor.get);
			add(descriptor.set);
		}
		add(Object.getPrototypeOf(function* () {}));
		add(Object.getPrototypeOf(async function () {}));
		add(Object.getPrototypeOf([][Symbol.iterator]()));
		add(Object.getPrototypeOf(new Map()[Symbol.iterator]()));
		add(Object.getPrototypeOf(""[Symbol.iterator]()));
		while (queue.length > 0) {
			const object = queue.pop();
			add(Object.getPrototypeOf(object));
			const descriptors = Object.getOwnPropertyDescriptors(object);
			for (const key of Reflect.ownKeys(descriptors)) {
				const descriptor = descriptors[key];
				if ("value" in descriptor) add(descriptor.value);
				else {
					add(descriptor.get);
					add(descriptor.set);
				}
			}
			Object.freeze(object);
		}
	})();

	const stringify = JSON.stringify;
	const parse = JSON.parse;
	const promiseThen = Promise.prototype.then;
	const ErrorCtor = Error;
	const TypeErrorCtor = TypeError;
	const RangeErrorCtor = RangeError;
	const pending = new Map();
	let nextId = 1;
	let finished = false;
	// Thrown by exit() to unwind the script after it already reported success.
	const EXIT = Object.freeze({});

	const config = parse(configJson);
	const limits = config.limits || {};
	const MAX_STORE_VALUE_CHARS = limits.maxStoreValueChars || 262144;
	const MAX_STORE_TOTAL_CHARS = limits.maxStoreTotalChars || 1048576;
	const MAX_OUTPUT_CHARS = limits.maxOutputChars || 16777216;
	const MAX_OUTPUT_ITEMS = limits.maxOutputItems || 100000;

	function done(ok, payload, writes) {
		if (finished) return;
		finished = true;
		bridge("done", ok, payload === undefined ? "" : payload, writes === undefined ? "" : writes);
	}

	function errorText(error) {
		const head = error && error.message ? String(error.name) + ": " + String(error.message) : String(error);
		const frames =
			error && typeof error.stack === "string"
				? error.stack.split("\n").filter((line) => line.trim() && line.indexOf("codemode-prelude") === -1)
				: [];
		return [head].concat(frames).join("\n");
	}

	function format(value) {
		if (typeof value === "string") return value;
		if (value instanceof ErrorCtor) return errorText(value);
		try {
			const json = stringify(value);
			return json === undefined ? String(value) : json;
		} catch (error) {
			return String(value);
		}
	}

	function describeError(error) {
		try {
			if (error instanceof ErrorCtor) {
				return stringify({ name: String(error.name), message: String(error.message), stack: errorText(error) });
			}
			return stringify({ message: format(error) });
		} catch (error) {
			return stringify({ message: "The script threw a value that cannot be described" });
		}
	}

	// A caller of a tool or a host global: arguments and results make a JSON round
	// trip, and the promise settles when the host answers.
	function caller(kind, name, spread) {
		return function () {
			const args = Array.prototype.slice.call(arguments);
			return new Promise((resolve, reject) => {
				let json;
				try {
					json = stringify(spread ? args : args[0] === undefined ? null : args[0]);
				} catch (error) {
					reject(error);
					return;
				}
				const id = nextId++;
				pending.set(id, { resolve, reject });
				bridge(kind, id, name, json === undefined ? "null" : json);
			});
		};
	}

	const tools = Object.create(null);
	const allTools = [];
	for (const tool of config.tools) {
		const fn = caller("call", tool.name, false);
		// The first tool wins when two names normalize to the same identifier.
		if (!(tool.jsName in tools)) {
			tools[tool.jsName] = fn;
			allTools.push(Object.freeze({ name: tool.jsName, description: tool.description }));
		}
		if (!(tool.name in tools)) tools[tool.name] = fn;
	}
	Object.freeze(tools);
	Object.freeze(allTools);

	// Reading a member that does not exist throws an error that names close
	// matches, instead of a later "not a function".
	const comparable = (name) => name.toLowerCase().replace(/[^a-z0-9]/g, "");
	function guard(target, label, names, hint) {
		return new Proxy(target, {
			get(object, property, receiver) {
				if (
					typeof property !== "string" ||
					property in object ||
					property in Object.prototype ||
					property === "then" ||
					property === "toJSON"
				) {
					return Reflect.get(object, property, receiver);
				}
				const wanted = comparable(property);
				const exact = names.filter((name) => comparable(name) === wanted);
				const close =
					exact.length > 0
						? exact
						: names.filter(
								(name) => wanted && (comparable(name).indexOf(wanted) !== -1 || wanted.indexOf(comparable(name)) !== -1),
							);
				let message = label + "." + property + " does not exist.";
				if (close.length > 0) {
					message +=
						" Did you mean " +
						close
							.slice(0, 5)
							.map((name) => label + "." + name)
							.join(", ") +
						"?";
				} else if (names.length <= 20) {
					message += " Available: " + names.join(", ") + ".";
				}
				if (hint) message += " " + hint;
				message += ' Check for a member with "' + property + '" in ' + label + ".";
				throw new TypeErrorCtor(message);
			},
		});
	}
	const toolsProxy = guard(
		tools,
		"tools",
		allTools.map((tool) => tool.name),
		"ALL_TOOLS lists every tool; searchTools(query) finds tools by topic.",
	);

	const namespaces = new Map();
	for (const global of config.globals) {
		const fn = caller("global", global.name, global.spread === true);
		const dot = global.name.indexOf(".");
		if (dot === -1) {
			Object.defineProperty(globalThis, global.name, { value: fn, enumerable: true });
			continue;
		}
		const namespace = global.name.slice(0, dot);
		if (!namespaces.has(namespace)) namespaces.set(namespace, Object.create(null));
		namespaces.get(namespace)[global.name.slice(dot + 1)] = fn;
	}
	for (const entry of namespaces) {
		const namespace = entry[0];
		const members = entry[1];
		Object.freeze(members);
		Object.defineProperty(globalThis, namespace, { value: guard(members, namespace, Object.keys(members)), enumerable: true });
	}

	// key -> JSON text. Sizes count key and JSON characters.
	const stored = new Map(Object.entries(config.store || {}));
	const writes = new Map();
	let storedChars = 0;
	for (const entry of stored) storedChars += entry[0].length + entry[1].length;

	const STORE_HINT =
		"store() is for small state such as IDs or summaries. Show images with image(), keep large data in variables, or write it to a file with a tool.";

	function checkKey(name, key) {
		if (typeof key !== "string") throw new TypeErrorCtor(name + "() key must be a string");
	}

	function store(key, value) {
		checkKey("store", key);
		const previous = stored.has(key) ? key.length + stored.get(key).length : 0;
		if (value === undefined) {
			stored.delete(key);
			storedChars -= previous;
			writes.set(key, undefined);
			return;
		}
		let json;
		try {
			json = stringify(value);
		} catch (error) {
			throw new TypeErrorCtor("store(" + stringify(key) + ") value is not JSON-serializable: " + format(error));
		}
		if (json === undefined) throw new TypeErrorCtor("store(" + stringify(key) + ") value is not JSON-serializable");
		if (json.length > MAX_STORE_VALUE_CHARS) {
			throw new RangeErrorCtor(
				"store(" +
					stringify(key) +
					") value has " +
					json.length +
					" characters of JSON, more than the limit of " +
					MAX_STORE_VALUE_CHARS +
					". " +
					STORE_HINT,
			);
		}
		const next = storedChars - previous + key.length + json.length;
		if (next > MAX_STORE_TOTAL_CHARS) {
			throw new RangeErrorCtor(
				"store is full: stored values would exceed " +
					MAX_STORE_TOTAL_CHARS +
					" characters of JSON. Delete keys with store(key, undefined). " +
					STORE_HINT,
			);
		}
		stored.set(key, json);
		storedChars = next;
		writes.set(key, json);
	}

	function load(key) {
		checkKey("load", key);
		const json = stored.get(key);
		return json === undefined ? undefined : parse(json);
	}

	function serializeWrites() {
		const entries = [];
		for (const entry of writes) entries.push(entry[1] === undefined ? [entry[0]] : [entry[0], entry[1]]);
		return stringify(entries);
	}

	Object.defineProperty(globalThis, "store", { value: store, enumerable: true });
	Object.defineProperty(globalThis, "load", { value: load, enumerable: true });

	let outputChars = 0;
	let outputItems = 0;

	// Past the output limits the script fails: done() reports the error, so
	// catching it does not resume output, and the host ends the script.
	function output(kind, data, mimeType) {
		if (finished) return;
		outputChars += data.length;
		outputItems++;
		if (outputChars > MAX_OUTPUT_CHARS || outputItems > MAX_OUTPUT_ITEMS) {
			const error = new RangeErrorCtor(
				"script output exceeded the limit of " +
					MAX_OUTPUT_CHARS +
					" characters or " +
					MAX_OUTPUT_ITEMS +
					" text(), image(), and console calls. Print a summary instead, or write large data to a file with a tool.",
			);
			done(false, describeError(error));
			throw error;
		}
		bridge("output", kind, data, mimeType === undefined ? "" : mimeType);
	}

	// Primitives become their string form, everything else JSON.
	function outputText(value) {
		if (value === undefined || value === null || (typeof value !== "object" && typeof value !== "function")) {
			return String(value);
		}
		const json = stringify(value);
		return json === undefined ? String(value) : json;
	}

	function text(value) {
		let rendered;
		try {
			rendered = outputText(value);
		} catch (error) {
			throw new TypeErrorCtor(error instanceof ErrorCtor ? error.message : String(error));
		}
		output("text", rendered);
	}

	const IMAGE_HELPER_EXPECTS =
		'image expects a non-empty image URL string, an object with image_url, or an image block { type: "image", data, mimeType }';

	function imageUrl(value) {
		if (typeof value === "string") return value;
		if (typeof value !== "object" || value === null || Array.isArray(value)) {
			throw new TypeErrorCtor(IMAGE_HELPER_EXPECTS);
		}
		if (value.image_url !== undefined) {
			if (typeof value.image_url !== "string") throw new TypeErrorCtor(IMAGE_HELPER_EXPECTS);
			return value.image_url;
		}
		if (typeof value.type !== "string") throw new TypeErrorCtor(IMAGE_HELPER_EXPECTS);
		if (value.type !== "image") throw new TypeErrorCtor('image only accepts image blocks, got "' + value.type + '"');
		if (typeof value.data !== "string" || value.data === "") throw new TypeErrorCtor("image expected image block data");
		if (value.data.toLowerCase().indexOf("data:") === 0) return value.data;
		return "data:;base64," + value.data;
	}

	// Base64 of the signatures of the formats providers accept inline (PNG, JPEG
	// except JPEG-LS, GIF, "RIFF....WEBP"). Signatures start at byte 0, so their
	// encodings are prefixes.
	const IMAGE_SIGNATURES = [
		["image/png", /^iVBORw0KGg/],
		["image/jpeg", /^[/]9j[/](?!9)/],
		["image/gif", /^R0lGOD[dl]h/],
		["image/webp", /^UklG.{8}RUJQ/],
	];

	function image(value) {
		const url = imageUrl(value);
		if (url === "") throw new TypeErrorCtor(IMAGE_HELPER_EXPECTS);
		const colon = url.indexOf(":");
		const scheme = colon === -1 ? "" : url.slice(0, colon).toLowerCase();
		if (scheme === "http" || scheme === "https") {
			throw new TypeErrorCtor("remote image URLs are not supported in tool outputs. Pass a base64 data URI instead");
		}
		const comma = url.indexOf(",");
		const header = comma === -1 ? [] : url.slice(colon + 1, comma).split(";");
		if (scheme !== "data" || comma === -1 || header.slice(1).every((part) => part.toLowerCase() !== "base64")) {
			throw new TypeErrorCtor("invalid image output. Pass a base64 data URI instead");
		}
		const data = url.slice(comma + 1).replace(/\s+/g, "");
		if (data.length % 4 !== 0 || !/^[A-Za-z0-9+/]+={0,2}$/.test(data)) {
			throw new TypeErrorCtor("invalid image output. The image data is not valid base64 (truncated or corrupted?)");
		}
		const head = data.slice(0, 16);
		let mimeType = "";
		for (const signature of IMAGE_SIGNATURES) {
			if (signature[1].test(head)) {
				mimeType = signature[0];
				break;
			}
		}
		if (mimeType === "") {
			throw new TypeErrorCtor("invalid image output. The image data is not a PNG, JPEG, GIF, or WebP image");
		}
		output("image", data, mimeType);
	}

	function exit() {
		let writesJson;
		try {
			writesJson = serializeWrites();
		} catch (error) {
			done(false, describeError(error));
			throw EXIT;
		}
		done(true, undefined, writesJson);
		throw EXIT;
	}

	const consoleObject = {};
	for (const level of ["log", "info", "warn", "error", "debug"]) {
		consoleObject[level] = function () {
			const parts = Array.prototype.slice.call(arguments).map(format);
			output("console", parts.join(" "));
		};
	}
	Object.freeze(consoleObject);

	Object.defineProperty(globalThis, "tools", { value: toolsProxy, enumerable: true });
	Object.defineProperty(globalThis, "ALL_TOOLS", { value: allTools, enumerable: true });
	Object.defineProperty(globalThis, "console", { value: consoleObject, enumerable: true });
	Object.defineProperty(globalThis, "text", { value: text, enumerable: true });
	Object.defineProperty(globalThis, "image", { value: image, enumerable: true });
	Object.defineProperty(globalThis, "exit", { value: exit, enumerable: true });

	return {
		settle(id, ok, payload, propertiesJson) {
			const entry = pending.get(id);
			if (!entry) return;
			pending.delete(id);
			if (!ok) {
				const error = new ErrorCtor(payload);
				if (propertiesJson !== undefined && propertiesJson !== "") {
					const properties = parse(propertiesJson);
					for (const key of Object.keys(properties)) {
						if (key === "name" || key === "message" || key === "stack") continue;
						Object.defineProperty(error, key, { value: properties[key], writable: true, enumerable: true, configurable: true });
					}
				}
				entry.reject(error);
				return;
			}
			let value;
			try {
				value = payload === undefined || payload === "" ? undefined : parse(payload);
			} catch (error) {
				entry.reject(error);
				return;
			}
			entry.resolve(value);
		},
		// Attach the result handlers to the promise the host already started. The
		// script is compiled by the host, not here: `new Function` would shift
		// every stack-trace line by two.
		track(promise) {
			promiseThen.call(
				promise,
				(value) => {
					let json;
					try {
						json = value === undefined ? undefined : stringify(value);
					} catch (error) {
						done(false, describeError(error));
						return;
					}
					let writesJson;
					try {
						writesJson = serializeWrites();
					} catch (error) {
						done(false, describeError(error));
						return;
					}
					done(true, json, writesJson);
				},
				(error) => {
					done(false, describeError(error));
				},
			);
		},
		stalled() {
			if (finished || pending.size > 0) return false;
			done(
				false,
				stringify({
					name: "Error",
					message:
						"The script is waiting on a promise that can never settle: no tool call is pending, and timers do not exist here.",
				}),
			);
			return true;
		},
	};
})"#;
