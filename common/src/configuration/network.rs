//  Copyright 2021, The Tari Project
//
//  Redistribution and use in source and binary forms, with or without modification, are permitted provided that the
//  following conditions are met:
//
//  1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following
//  disclaimer.
//
//  2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the
//  following disclaimer in the documentation and/or other materials provided with the distribution.
//
//  3. Neither the name of the copyright holder nor the names of its contributors may be used to endorse or promote
//  products derived from this software without specific prior written permission.
//
//  THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES,
//  INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
//  DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
//  SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
//  SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY,
//  WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE
//  USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

use crate::ConfigurationError;
use std::{
    fmt,
    fmt::{Display, Formatter},
    str::FromStr,
};

/// Represents the available Tari p2p networks. Only nodes with matching byte values will be able to connect, so these
/// should never be changed once released.
#[repr(u8)]
#[derive(Clone, Debug, PartialEq, Eq, Copy)]
pub enum Network {
    MainNet = 0x00,
    LocalNet = 0x10,
    Ridcully = 0x21,
    Stibbons = 0x22,
    Weatherwax = 0x23,
}

impl Network {
    pub fn as_byte(self) -> u8 {
        self as u8
    }

    pub const fn as_str(self) -> &'static str {
        use Network::*;
        match self {
            MainNet => "mainnet",
            Ridcully => "ridcully",
            Stibbons => "stibbons",
            Weatherwax => "weatherwax",
            LocalNet => "localnet",
        }
    }
}

impl Default for Network {
    fn default() -> Self {
        Network::MainNet
    }
}

impl FromStr for Network {
    type Err = ConfigurationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        use Network::*;
        match value.to_lowercase().as_str() {
            "ridcully" => Ok(Ridcully),
            "stibbons" => Ok(Stibbons),
            "weatherwax" => Ok(Weatherwax),
            "mainnet" => Ok(MainNet),
            "localnet" => Ok(LocalNet),
            invalid => Err(ConfigurationError::new(
                "network",
                &format!("Invalid network option: {}", invalid),
            )),
        }
    }
}

impl Display for Network {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
