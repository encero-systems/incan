# A minimal stdio LSP peer: Content-Length frames, server requests answered with null, versioned diagnostics.
require_relative 'support'

module HeadToHead
  class LspClient
    attr_reader :marks

    def initialize(argv, environment, directory, stderr_path, timeout_seconds)
      @deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + timeout_seconds
      @stdin, @stdout, @stderr, @waiter = Open3.popen3(environment, *argv, chdir: directory)
      @stdout.binmode
      @drain = Thread.new { File.open(stderr_path, 'wb') { |log| IO.copy_stream(@stderr, log) } }
      @buffer = String.new(encoding: Encoding::BINARY)
      @pending = []
      @next_id = 0
      @started = HeadToHead.clock_ns
      @marks = {}
    end

    # Milliseconds since spawn at a named protocol point.
    def mark(name)
      @marks[name] = (HeadToHead.clock_ns - @started) / 1_000_000.0
    end

    # A null `params` is omitted, as the Contracts peer omits it; the server refuses an explicit null.
    def notify(method, params)
      write_frame({ jsonrpc: '2.0', method: method, params: params }.compact)
    end

    def request(method, params)
      identifier = (@next_id += 1)
      write_frame({ jsonrpc: '2.0', id: identifier, method: method, params: params }.compact)
      loop do
        message = receive
        unless message['id'] == identifier && !message.key?('method')
          @pending << message if message['method'] == 'textDocument/publishDiagnostics'
          next
        end

        HeadToHead.refuse("LSP #{method} failed", message) if message.key?('error')
        return message['result']
      end
    end

    # The diagnostics the server published for exactly this document version.
    def diagnostics(uri, version)
      loop do
        found = @pending.find { |message| diagnostics_for?(message, uri, version) }
        if found
          @pending.delete(found)
          return found.fetch('params').fetch('diagnostics')
        end
        @pending << receive
      end
    end

    # Close stdin after the exit notification and reap the server.
    def finish
      @stdin.close unless @stdin.closed?
      status = @waiter.value
      @drain.join
      status
    end

    private

    def diagnostics_for?(message, uri, version)
      params = message['params']
      message['method'] == 'textDocument/publishDiagnostics' && params['uri'] == uri && params['version'] == version
    end

    def write_frame(message)
      body = JSON.generate(message)
      @stdin.write("Content-Length: #{body.bytesize}\r\n\r\n#{body}")
      @stdin.flush
    end

    # One server message. Server-to-client requests get a null result so the server never waits on this client.
    def receive
      header = read_until("\r\n\r\n")
      length = header[/Content-Length:\s*(\d+)/i, 1]
      HeadToHead.refuse('LSP frame has no Content-Length', { header: header }) unless length
      message = JSON.parse(read_bytes(Integer(length)))
      if message.key?('id') && message.key?('method')
        write_frame({ jsonrpc: '2.0', id: message['id'], result: nil })
      elsif message.key?('method') && message['method'] != 'textDocument/publishDiagnostics'
        return receive
      end
      message
    end

    def read_until(separator)
      fill until (index = @buffer.index(separator))
      @buffer.slice!(0, index + separator.bytesize)
    end

    def read_bytes(count)
      fill while @buffer.bytesize < count
      @buffer.slice!(0, count).force_encoding(Encoding::UTF_8)
    end

    def fill
      remaining = @deadline - Process.clock_gettime(Process::CLOCK_MONOTONIC)
      HeadToHead.refuse('LSP reply timed out') if remaining <= 0 || IO.select([@stdout], nil, nil, remaining).nil?
      @buffer << @stdout.readpartial(65_536)
    rescue EOFError
      HeadToHead.refuse('LSP server closed its output early')
    end
  end
end
