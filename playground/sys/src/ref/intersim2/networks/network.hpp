// $Id: network.hpp 5188 2012-08-30 00:31:31Z dub $

/*
 Copyright (c) 2007-2012, Trustees of The Leland Stanford Junior University
 All rights reserved.

 Redistribution and use in source and binary forms, with or without
 modification, are permitted provided that the following conditions are met:

 Redistributions of source code must retain the above copyright notice, this
 list of conditions and the following disclaimer.
 Redistributions in binary form must reproduce the above copyright notice, this
 list of conditions and the following disclaimer in the documentation and/or
 other materials provided with the distribution.

 THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
 ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
 WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
 DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR
 ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
 (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
 LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
 ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
 (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
 SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
*/

#ifndef _NETWORK_HPP_
#define _NETWORK_HPP_

#include <deque>
#include <vector>

#include "../channel.hpp"
#include "../config_utils.hpp"
#include "../credit.hpp"
#include "../flit.hpp"
#include "../flitchannel.hpp"
#include "../globals.hpp"
#include "../module.hpp"
#include "../routers/router.hpp"
#include "../timed_module.hpp"

typedef Channel<Credit> CreditChannel;

class InterconnectInterface;

class Network : public TimedModule {
 protected:
  int _size;
  int _nodes;
  int _channels;
  int _classes;

  std::vector<Router *> _routers;

  std::vector<FlitChannel *> _inject;
  std::vector<CreditChannel *> _inject_cred;

  std::vector<FlitChannel *> _eject;
  std::vector<CreditChannel *> _eject_cred;

  std::vector<FlitChannel *> _chan;
  std::vector<CreditChannel *> _chan_cred;

  std::deque<TimedModule *> _timed_modules;

  virtual void _ComputeSize(const Configuration &config) = 0;
  virtual void _BuildNet(const Configuration &config) = 0;

  void _Alloc();

 public:
  Network(const Configuration &config, const std::string &name,
          InterconnectInterface *icnt);
  virtual ~Network();

  static Network *New(const Configuration &config, const std::string &name,
                      InterconnectInterface *icnt);

  virtual void WriteFlit(Flit *f, int source);
  virtual Flit *ReadFlit(int dest);

  virtual void WriteCredit(Credit *c, int dest);
  virtual Credit *ReadCredit(int source);

  inline int NumNodes() const { return _nodes; }

  virtual void InsertRandomFaults(const Configuration &config);
  void OutChannelFault(int r, int c, bool fault = true);

  virtual double Capacity() const;

  virtual void ReadInputs();
  virtual void Evaluate();
  virtual void WriteOutputs();

  void Display(std::ostream &os = std::cout) const;
  void DumpChannelMap(std::ostream &os = std::cout,
                      std::string const &prefix = "") const;
  void DumpNodeMap(std::ostream &os = std::cout,
                   std::string const &prefix = "") const;

  int NumChannels() const { return _channels; }
  const std::vector<FlitChannel *> &GetInject() { return _inject; }
  FlitChannel *GetInject(int index) { return _inject[index]; }
  const std::vector<CreditChannel *> &GetInjectCred() { return _inject_cred; }
  CreditChannel *GetInjectCred(int index) { return _inject_cred[index]; }
  const std::vector<FlitChannel *> &GetEject() { return _eject; }
  FlitChannel *GetEject(int index) { return _eject[index]; }
  const std::vector<CreditChannel *> &GetEjectCred() { return _eject_cred; }
  CreditChannel *GetEjectCred(int index) { return _eject_cred[index]; }
  const std::vector<FlitChannel *> &GetChannels() { return _chan; }
  const std::vector<CreditChannel *> &GetChannelsCred() { return _chan_cred; }
  const std::vector<Router *> &GetRouters() { return _routers; }
  Router *GetRouter(int index) { return _routers[index]; }
  int NumRouters() const { return _size; }
};

#endif
