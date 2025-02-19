const { spawn } = require("child_process");
const { expect } = require("chai");
const fs = require("fs");
const path = require("path");
const BaseNodeClient = require("./baseNodeClient");
const { sleep, getFreePort } = require("./util");
const dateFormat = require("dateformat");
const { createEnv } = require("./config");

let outputProcess;
class BaseNodeProcess {
  constructor(name, excludeTestEnvars, options, logFilePath, nodeFile) {
    this.name = name;
    this.logFilePath = logFilePath ? path.resolve(logFilePath) : logFilePath;
    this.nodeFile = nodeFile;
    this.options = options;
    this.excludeTestEnvars = excludeTestEnvars;
  }

  async init() {
    this.port = await getFreePort(19000, 25000);
    this.grpcPort = await getFreePort(19000, 25000);
    this.name = `Basenode${this.port}-${this.name}`;
    this.nodeFile = this.nodeFile || "nodeid.json";

    do {
      this.baseDir = `./temp/base_nodes/${dateFormat(
        new Date(),
        "yyyymmddHHMM"
      )}/${this.name}`;
      // Some tests failed during testing because the next base node process started in the previous process
      // directory therefore using the previous blockchain database
      if (fs.existsSync(this.baseDir)) {
        sleep(1000);
      }
    } while (fs.existsSync(this.baseDir));
    const args = ["--base-path", ".", "--init", "--create-id"];
    if (this.logFilePath) {
      args.push("--log-config", this.logFilePath);
    }

    await this.run(await this.compile(), args);
    // console.log("Port:", this.port);
    // console.log("GRPC:", this.grpcPort);
    // console.log(`Starting node ${this.name}...`);
  }

  async compile() {
    if (!outputProcess) {
      await this.run("cargo", [
        "build",
        "--release",
        "--bin",
        "tari_base_node",
        "-Z",
        "unstable-options",
        "--out-dir",
        process.cwd() + "/temp/out",
      ]);
      outputProcess = process.cwd() + "/temp/out/tari_base_node";
    }
    return outputProcess;
  }

  ensureNodeInfo() {
    for (;;) {
      if (fs.existsSync(this.baseDir + "/" + this.nodeFile)) {
        break;
      }
    }

    this.nodeInfo = JSON.parse(
      fs.readFileSync(this.baseDir + "/" + this.nodeFile, "utf8")
    );
  }

  peerAddress() {
    this.ensureNodeInfo();
    const addr = this.nodeInfo.public_key + "::" + this.nodeInfo.public_address;
    // console.log("Peer:", addr);
    return addr;
  }

  setPeerSeeds(addresses) {
    this.peerSeeds = addresses.join(",");
  }

  getGrpcAddress() {
    const address = "127.0.0.1:" + this.grpcPort;
    // console.log("Base Node GRPC Address:",address);
    return address;
  }

  run(cmd, args) {
    return new Promise((resolve, reject) => {
      if (!fs.existsSync(this.baseDir)) {
        fs.mkdirSync(this.baseDir, { recursive: true });
        fs.mkdirSync(this.baseDir + "/log", { recursive: true });
      }

      let envs = [];
      if (!this.excludeTestEnvars) {
        envs = createEnv(
          this.name,
          false,
          this.nodeFile,
          "127.0.0.1",
          "8082",
          "8081",
          "127.0.0.1",
          this.grpcPort,
          this.port,
          "127.0.0.1:8080",
          this.options,
          this.peerSeeds
        );
      }

      const ps = spawn(cmd, args, {
        cwd: this.baseDir,
        // shell: true,
        env: { ...process.env, ...envs },
      });

      ps.stdout.on("data", (data) => {
        //console.log(`stdout: ${data}`);
        fs.appendFileSync(`${this.baseDir}/log/stdout.log`, data.toString());
        if (
          // Make this resilient by comparing uppercase and making provisioning that the first print message in the
          // base node console is not always 'State: Starting up'
          data
            .toString()
            .toUpperCase()
            .match(/STATE: STARTING/) ||
          data
            .toString()
            .toUpperCase()
            .match(/STATE: LISTENING/) ||
          data
            .toString()
            .toUpperCase()
            .match(/STATE: SYNCING/)
        ) {
          resolve(ps);
        }
      });

      ps.stderr.on("data", (data) => {
        console.error(`stderr: ${data}`);
        fs.appendFileSync(`${this.baseDir}/log/stderr.log`, data.toString());
      });

      ps.on("close", (code) => {
        const ps = this.ps;
        this.ps = null;
        if (code) {
          console.log(`child process exited with code ${code}`);
          reject(`child process exited with code ${code}`);
        } else {
          resolve(ps);
        }
      });

      expect(ps.error).to.be.an("undefined");
      this.ps = ps;
    });
  }

  async startNew() {
    await this.init();
    const start = await this.start();
    return start;
  }

  async startAndConnect() {
    await this.startNew();
    return this.createGrpcClient();
  }

  async start() {
    const args = ["--base-path", "."];
    if (this.logFilePath) {
      args.push("--log-config", this.logFilePath);
    }
    return await this.run(await this.compile(), args);
  }

  stop() {
    return new Promise((resolve) => {
      if (!this.ps) {
        return resolve();
      }
      this.ps.on("close", (code) => {
        if (code) {
          console.log(`child process exited with code ${code}`);
        }
        resolve();
      });
      this.ps.kill("SIGINT");
    });
  }

  createGrpcClient() {
    return new BaseNodeClient(this.grpcPort);
  }
}

module.exports = BaseNodeProcess;
